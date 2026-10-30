# Reaching an app's native UI

## What it is

How `native list`, `native choose`, `native dismiss`, `native tray`, `native pick` and `native answer` read and operate a launched app's menu bars, context menus, tray icons, file dialogs and dialogs on Windows, without taking the user's focus, and how pokit gives the foreground back when what it set off in the app takes it.

## Governing decisions

**None.** [ADR-0001](../adr/0001-commands-talk-to-a-session.md) is adjacent: an attached session, which it allows, does not know the app's process, so `native` refuses there; the ADR decides where commands run, not how native UI is reached.

## Design model

The rules come from maintainer's calls on #5 and #56, the give-back the maintainer chose for launch (#1 story 42), and otherwise from the source. Outside input: a menu path (`File > Say hello`, split on `>` by `parse_path`; an empty one is a usage error, exit 2), a button name, a dialog title, a file path, a tray index. What is read back is bounded by the app: its menus, its `#32770` windows, its `tray_icon_app` windows; nothing caps their count or a menu's depth (`read_menu` recurses into every submenu).

Failures: what was named is not there, exit 5 (`native::Refusal::Missing`); it is there and cannot be operated, exit 1 (`Refusal::Refused`); an attached session, exit 7 (`State::native_pid`).

### Menu bars

- **Menu bars are read and chosen through Win32, not UI Automation.** Tauri's menus (muda 0.20.0) are ordinary Win32 menus: `CreateMenu` and `SetMenu` build them, and a click arrives as `WM_COMMAND` with the entry's id (`muda` `platform_impl/windows/mod.rs` 245, 280, 1051). pokit reads the tree with `GetMenu` / `GetMenuItemInfoW` without opening anything (`native::menus`, `read_menu`), and chooses an entry by posting that `WM_COMMAND` (`native::choose`). So it needs no focus and opens no menu. A disabled entry, an entry that only opens a submenu, and any entry of a window that is disabled are refused (exit 1); a path no menu has is not found (exit 5). A window is disabled while one of its modal dialogs is open, and then a user cannot reach its menu; a posted `WM_COMMAND` still would, running the command in the middle of the app's modal state, so pokit checks `IsWindowEnabled` first. **Maintainer's call (2026-10-06, #5):** menus through Win32, dialogs and reading through UI Automation. The alternatives shown were all UI Automation, which opens menus and so needs the window in front, and all Win32, which cannot read custom-drawn UI or the file dialog.
- **Labels are shown as the user sees them.** The `&` access marks are dropped (`&&` stays `&`), and the shortcut after a tab is split off: `Say &hello\tCtrl+H` is listed as `Say hello (Ctrl+H)` and chosen as `File > Say hello` (`clean_label`, `render`). Pinned by `labels_lose_their_access_keys_and_keep_their_shortcut_apart` and `a_menu_renders_one_entry_per_line_with_its_state` (native.rs unit tests).

### Dialogs

- **Dialogs are the app's `#32770` windows, read through UI Automation** (`native::dialogs`). Their text and buttons come from the element tree; buttons in the title bar, such as Close, are left out (`in_title_bar`).
- **A Win32 button is pressed by posting the dialog the `WM_COMMAND` its click sends** (`native::answer`, `BN_CLICKED`). UI Automation's Invoke on a Win32 button sends `BM_CLICK`, which "might fail" when the dialog is not active (`BM_CLICK` remarks, win32 docs, read 2026-10-06). The fix those docs give, `SetActiveWindow`, would take the user's focus. Measured: pressed as soon as the dialog appeared, Invoke left it open 4 times out of 5, and the posted `WM_COMMAND(id, BN_CLICKED)` closed it 5 times out of 5 (2026-10-06). Buttons of other kinds still go through Invoke. Before pressing, pokit waits until the dialog's thread takes a `WM_NULL` (`SendMessageTimeout`, 1 s). Afterwards it reports `closed`, from whether the dialog's window is gone within 1 s. A button that does not close its dialog is not pressed again.
- **A localised button answers to its name without the access key** (`button_matches`). On a Korean system the fixture's buttons are `예(Y)` and `아니요(N)`, and `native answer 예` presses the first. `native list` gives the full names, and `tests/native.rs` takes button names from it, because they follow the system's language. Pinned by `a_localised_button_answers_to_its_name_without_the_access_key`.
- **A page's own `alert`, `confirm` and `prompt` are not native UI here.** WebView2 draws them in its own browser process, not as the app's `#32770` windows, so `native` does not see them.

### Giving the foreground back

- **What pokit sets off in the app does not keep the foreground.** A dialog activates itself when it opens, and the app regains the foreground when it closes, both with the right inherited from the terminal in front ([Launching a test instance](launching-a-test-instance.md)). Measured 2026-10-06: after `native choose "Help > Ask"`, the fixture held the foreground for about 2.1 s. So from the start of a menu-bar `native choose` and of `native answer` until 2 s after each returns (`NATIVE_FOCUS_GRACE`), the session gives the foreground back whenever the app takes it (`State::keep_foreground_after`, which runs launch's `keep_foreground`, [Launching a test instance](launching-a-test-instance.md)); after that change the fixture never showed up in front. *Derivation, not a maintainer's call:* this extends the launch give-back the maintainer chose (#1 story 42) to the native commands, with the same best-effort limit. A refused give-back is logged once until one succeeds, not every 50 ms (`keep_foreground`).
- **A context menu takes the foreground, and pokit gives it back after `native choose` or `native dismiss` closes it.** muda calls `SetForegroundWindow` before `TrackPopupMenu`, so a right-click that opens the menu, even over CDP, brings the app to the front when Windows lets it. **Maintainer's call (2026-10-07, #5):** give the foreground back, as after a dialog, rather than refuse context menus unless the app was activated first, or offer both.
  - The window given back to is the one in front at the last CDP click (`foreground_at_click`, set in `State::click_cmd` when another process's window is in front), since by the time of the choice the app is (`State::foreground_before_menu`). It is used only when the app is in front, and forgotten once a choice, dismiss or pick has succeeded, so a refused choice does not lose it; `native answer` and a menu-bar choice give back to the window in front when they start. `window activate` forgets it too, so a deliberate activation is not undone ([Sending OS input](sending-os-input.md)).
  - The session's give-back starts only after the menu has closed: taking the foreground from the app while its menu is open could close the menu mid-choice.
  - The session could not do it alone: after the first time, Windows refused its `SetForegroundWindow` 66 times in a row, with or without attaching to the app's input. A process started from the terminal in front could, at once. So the session hands the window back in its answer (`give_back`, `State::give_back_field`, only while the app holds the foreground), and the command line, started from that terminal, sets it, retrying every 50 ms for up to a second (`crates/pokit/src/main.rs — give_back`); the result says `foreground_given_back`.
  - Measured 2026-10-07 over six right-clicks in one session: the app took the foreground at the first two and was given it back each time; at the next four Windows did not let it take the foreground at all.
- **`capabilities` says it** for `native`: "a context or tray menu or a file dialog the app opens holds the foreground while it is open; `native choose`, `native dismiss` and `native pick` give it back", and `tray`: "icons made with tray-icon (Tauri's tray) only" (`crates/pokit/src/main.rs — capabilities`).
- Pinned by `a_dialog_the_app_opens_does_not_keep_the_foreground` (tests/native.rs): it checks that the foreground is where it was after `native choose "Help > Ask"`, and prints whether the app took it, read from the session's log; a run where Windows kept the app back tests nothing about the give-back. Reddened 2026-10-08: a give-back that reports success without calling `SetForegroundWindow` turned it red 2 times out of 2; no give-back at all, 1 time out of 2 (the other run, the app did not take the foreground and the test said so). Every focus test here observes rather than predicts whether the app took the foreground ([Launching a test instance](launching-a-test-instance.md), #48).

### Context menus

- **A context menu is read from its menu window, and chosen in by the keyboard's messages.** The app opens it (the fixture on a right-click, through `popup_menu`); it is a `#32768` window of the app, and `MN_GETHMENU` gives the menu it shows, read like a menu bar (`menu_windows`, `context_menu`). `native list` reports it as `context_menu`.
  - muda shows it with `TPM_RETURNCMD` and acts on the id `TrackPopupMenu` returns (`muda` 0.20.0 `platform_impl/windows/mod.rs` 906–936), so a posted `WM_COMMAND`, as for a menu bar, does nothing: measured, the entry did not run. UI Automation found no entries under the menu window either.
  - pokit highlights each entry on the path with `MN_SELECTITEM` (its position, separators counted), opens a submenu with a posted Right key, and takes the last with a posted Enter, as the keyboard would (`choose_in_context_menu`). It works with the app behind (measured 2026-10-07).
  - While a context menu is open, `native choose` chooses in it, and a path that is not in it is not found (exit 5) rather than chosen in the menu bar.
  - A menu bar's dropped-down menu is also a `#32768` window; it is told apart by its menu being one of the bar's submenus, and left out (`menu_windows`).
  - The first level is the menu window whose menu is no other open menu's submenu (`root_menu`), and a submenu's window is the one showing that entry's submenu, already open or opened with Right; neither relies on the windows' order. Each entry on the path must be enabled.
  - `native dismiss` closes it with Escape, a level at a time (`native::dismiss`).
- **A page's own context menu is not native UI.** WebView2 draws it in its browser process; only a menu the app opens, after the page prevents the default, is the app's. A CDP right-click closes WebView2's at once; that, with `native::browser_menus` and `native::close_browser_menu`, is [Sending input over CDP](sending-input-over-cdp.md)'s.
- Pinned by `a_context_menu_is_read_chosen_in_and_dismissed` (tests/native.rs): it checks the foreground came back only when the app took it at the menu's opening, observed rather than predicted, and so tests nothing about it in a run where Windows kept the app back at both openings (it prints which); it holds `FOCUS` with the dialog's focus test so neither moves the foreground under the other. Reddened 2026-10-07: falling through to the menu bar, Escape for Enter, no Escape on dismiss, choosing a disabled entry, and dropping the command line's give-back each turned it red.

### Tray icons

- **A tray icon is clicked by sending its own window the message the shell sends on a click, not by clicking the taskbar.** tray-icon, the crate behind Tauri's tray, makes a hidden `tray_icon_app` window for each icon and acts on its callback message `WM_USER_TRAYICON` (6002) with the mouse message as `lparam` (tray-icon 0.25.1 `platform_impl/windows/mod.rs` 43, 389–508). `native tray` posts that message (`TRAY_CLASS`, `TRAY_CALLBACK`, `click_tray`) with the down and up of a left or right click, or for a double click down, up, double-click, up.
  - So only tray icons made with tray-icon are reached; another app's own `Shell_NotifyIcon` callback message is its own, and pokit says it found no tray icon. `native list` gives how many there are (`tray_icons`); their tooltips live in the shell and are not read.
  - pokit cannot see whether the app acted on the click. tray-icon drops it silently when the shell gives no rectangle for the icon (`Shell_NotifyIconGetRect`), as for an icon the app hid; the message was still posted, so `native tray` reports the click. The app's effect is the evidence.
  - `--index` follows the order Windows lists the icons' windows (`EnumWindows` in `all_windows_of`), the most recently raised first, which can change between calls; there is no tooltip or id to pick an icon by.
  - tray-icon places its menu, and its event's position, at the user's real cursor (`GetCursorPos`), wherever that is.
  - A left click reached the fixture's click handler with the app behind, and the foreground stayed where it was. A right click opens the icon's menu, which tray-icon shows with `SetForegroundWindow` and `TrackPopupMenu` (`show_tray_menu`); it is a context menu like any other, read and chosen as above, and the foreground is given back the same way, the window in front at the click kept by `State::native_tray_cmd`.
  - The fixture adds its tray icon only with `--tray`. It was gated on the belief that `pokit close`, which ends the app by its job so the app never removes its icon, leaves a dead icon until the mouse passes over it; that does not hold on Windows 11 25H2 (Reference behaviour). **Maintainer's call (2026-10-13, #56):** the gate is kept and `close` is unchanged; the alternative shown was keeping #56 open for earlier builds.
- Pinned by `the_tray_icon_is_clicked_and_its_menu_chosen_in` (tests/native.rs): a left click reaches the app without moving the foreground; a right click opens the menu, whose entry is chosen. Reddened 2026-10-07: sending only the button's down, sending a left click for a right one, and not finding the icon's window each turned it red. Its give-back check, like the context menu test's, checks something only when the app took the foreground at the menu.

### File dialogs

- **A file is picked in a file dialog as typing its path and pressing Open would.** A file dialog is a `#32770` with a file name box, the edit control of id 1148 (`cmb13`, `FILE_NAME_BOX`), in the common item dialog Tauri's dialog plugin opens; the older dialogs were not tried, and may number the box differently. `native list` marks it `file`, and a folder picker would be marked too. `native pick PATH` sets that box's text (`WM_SETTEXT`) and posts the dialog the `WM_COMMAND` of `IDOK`, as `native answer` presses a button, so it works with the dialog behind (`native::pick`). The path is made absolute from where the command ran (`crates/pokit/src/main.rs — absolute`). More than one file dialog open without `--dialog` is refused (exit 1).
  - pokit then waits up to 3 s for the dialog to close, and otherwise says it stayed open, as an Open dialog may when given a path that does not exist (not tried).
  - The dialog takes the foreground when the app opens it, as any dialog does; `native pick` gives it back to the window in front at the click that opened it, like a context menu.
  - Measured 2026-10-07 on a Korean system: the plugin's dialog listed 28 buttons through UI Automation (toolbar, scroll and navigation buttons among them, `열기` twice), so `native list` is long for one; `native pick` does not use the buttons.
- Pinned by `a_file_is_picked_in_the_apps_file_dialog` (tests/native.rs): with no dialog open `pick` is not found; with the fixture's open dialog, a file whose name has a space is picked, the page shows its path, and the dialog closes. Reddened 2026-10-07: the wrong control id, the Cancel button's id for Open's, and another path typed each turned it red. Its give-back check, like the others', checks something only when the app took the foreground.

### Attached sessions

- **An attached session cannot reach the native UI** (exit 7). It does not know the app's process, and the native UI is found by process. Pinned by `an_attached_session_cannot_reach_the_native_ui` (tests/native.rs).
- The foreground tests run only where `quiet_machine` lets them ([Testing against the fixture](testing-against-the-fixture.md)).

## Code

- `crates/pokit/src/native.rs — MenuItem, Dialog, Answered, Refusal, clean_label, parse_path, find, button_matches, render, menus, read_menu, choose, choose_in_context_menu, context_menu, context_menu_open, menu_windows, root_menu, submenus, dismiss, click_tray, tray_icons, tray_windows, pick, file_name_box, dialogs, answer, in_title_bar, automation, windows_of, all_windows_of, MN_GETHMENU, MN_SELECTITEM, TRAY_CLASS, TRAY_CALLBACK, FILE_NAME_BOX, IDOK, BN_CLICKED`
- `crates/pokit/src/session/native.rs — State::native_pid, State::native_list_cmd, State::native_choose_cmd, State::native_answer_cmd, State::native_dismiss_cmd, State::native_pick_cmd, State::native_tray_cmd, State::keep_foreground_after, State::foreground_before_menu, State::give_back_field, NATIVE_FOCUS_GRACE, blocking`
- `crates/pokit/src/main.rs — give_back` (the command line's side of the give-back)

## Reference behaviour

- muda 0.20.0 `platform_impl/windows/mod.rs` 245, 280, 1051 (menu bars are Win32 menus; a click is `WM_COMMAND` with the id) and 906–936 (`TPM_RETURNCMD` for context menus); tray-icon 0.25.1 `platform_impl/windows/mod.rs` 43, 389–508 (`WM_USER_TRAYICON` 6002); `BM_CLICK` remarks, win32 docs, read 2026-10-06.
- **The taskbar was measured as the way to a tray icon and refused (2026-10-07, Windows 11):** through UI Automation on `Shell_TrayWnd`, the fixture's new icon was in the hidden-icons overflow and was not in the taskbar's tree at all, so it could not be found without opening the overflow, which would take the user's focus.
- **`close` does not leave a dead tray icon on Windows 11 25H2 (build 26200, 2026-10-13, #56):** with the icon in the overflow it was listed while the fixture ran and gone the next time the overflow opened after `close`; pinned to the taskbar (`IsPromoted`), UI Automation on `Shell_TrayWnd` found it while the fixture ran and not at all right after `close`, 3 s and 10 s later (2 of 2), and a screenshot showed it gone 2 s after.
- **Two ways to remove the icon before the job ends, measured for #56 and not used:** `NIM_DELETE` sent from another process for the app's `tray_icon_app` window removes a live app's icon (the icon's id, tray-icon's counter from 1, is found with `Shell_NotifyIconGetRect`), and `WM_CLOSE` posted to the `Tauri Window` alone makes the fixture exit through Tauri's own exit path, which drops its tray icons, in 44–78 ms (3 of 3). Posted also to tao's `Tao Thread Event Target` window, it destroyed the event loop's window and the app kept running with its icon. An app that keeps running in its tray when its window closes would not exit on `WM_CLOSE`.

## Cross-cutting invariants

- [Capabilities match behaviour](capabilities-match-behaviour.md) — `native` declares its `takes_focus` text (which menus and dialogs hold the foreground, and which commands give it back) and the `tray` limit.

## Blast radius

- [Launching a test instance](launching-a-test-instance.md) — the native give-back runs launch's `keep_foreground`; a change to its polling, its refusal logging or what it counts as the app holding the foreground changes every native command's give-back.
- [Sending input over CDP](sending-input-over-cdp.md) — a CDP click records `foreground_at_click`, which the context-menu give-back reads; and WebView2's own menu (closed there) and the app's `#32768` menu (read here) are told apart by process and class, so a change to either detection can touch the other's menu.
- [Sending OS input](sending-os-input.md) — `window activate` clears `foreground_at_click`; and the guard refuses OS input while a dialog `native` opened is in front, which assumes such a dialog is owned by the page's window.
- [Capturing a window with its native UI](capturing-a-window.md) — it draws the same `#32770` and `#32768` windows `native` reads; a menu or dialog `native` reaches is expected to appear in the capture.
- [Platform support](platform-support.md) — `capabilities` carries `native`'s `takes_focus` text and `tray` limit; a change to which commands give the foreground back, or to which tray icons are reached, makes them false.
- [Testing against the fixture](testing-against-the-fixture.md) — the fixture's menu, `Help > Ask` dialog, `popup_menu` context menu, `--tray` icon and file dialog are what these rules were measured against; changing them changes what the tests prove.

## Known holes / open

- Not covered: a save dialog's overwrite question, multi-select, and choosing a folder; none was tried. An Open dialog given a path that does not exist was not tried either.
- Not tested: a menu bar's dropped-down menu being left out of the context menu (dropping a bar menu down needs a click on the app in front).
- The older (non item) file dialogs were not tried, and may number the file name box differently.
- Tray icons on Windows builds before 25H2 were not measured for a dead icon after `close`, and whether a tray icon in every launch would change any test was not measured (#56).
- Nothing caps how many menus, entries, dialogs or dialog elements `native list` reads.
- `native` exists only on Windows; macOS refuses it as not built. Tracked: #119.

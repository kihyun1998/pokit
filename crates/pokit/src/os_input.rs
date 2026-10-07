//! The app as the OS sees it: which window is in front, bringing the app there, and input sent
//! through the OS (`SendInput`) instead of CDP.

#![cfg_attr(not(windows), allow(dead_code))]

/// How `activate` got the app to the front.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activation {
    /// It already was.
    AlreadyFront,
    /// `SetForegroundWindow` alone.
    Plain,
    /// `SetForegroundWindow` while attached to the foreground window's input queue.
    AttachInput,
}

/// Why OS input was not sent, or not all of it.
#[derive(Debug, Clone, PartialEq)]
pub enum OsError {
    /// The input would not reach the page: the app, or that page, is not in front. Nothing sent.
    NotFront(String),
    /// Something else went wrong.
    Failed(String),
}

/// A mouse button and how many presses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub right: bool,
    pub count: u32,
    pub hover: bool,
}

/// A physical key's scan code on a US keyboard (set 1), and whether it is an extended key; the
/// keys `chord` knows, by their `KeyboardEvent.code`.
pub fn us_scan(code: &str) -> Option<(u16, bool)> {
    const NAMED: [(&str, u16); 52] = [
        ("Escape", 0x01),
        ("Digit1", 0x02),
        ("Digit2", 0x03),
        ("Digit3", 0x04),
        ("Digit4", 0x05),
        ("Digit5", 0x06),
        ("Digit6", 0x07),
        ("Digit7", 0x08),
        ("Digit8", 0x09),
        ("Digit9", 0x0A),
        ("Digit0", 0x0B),
        ("Minus", 0x0C),
        ("Equal", 0x0D),
        ("Backspace", 0x0E),
        ("Tab", 0x0F),
        ("BracketLeft", 0x1A),
        ("BracketRight", 0x1B),
        ("Enter", 0x1C),
        ("ControlLeft", 0x1D),
        ("Semicolon", 0x27),
        ("Quote", 0x28),
        ("Backquote", 0x29),
        ("ShiftLeft", 0x2A),
        ("Backslash", 0x2B),
        ("Comma", 0x33),
        ("Period", 0x34),
        ("Slash", 0x35),
        ("AltLeft", 0x38),
        ("Space", 0x39),
        ("F1", 0x3B),
        ("F2", 0x3C),
        ("F3", 0x3D),
        ("F4", 0x3E),
        ("F5", 0x3F),
        ("F6", 0x40),
        ("F7", 0x41),
        ("F8", 0x42),
        ("F9", 0x43),
        ("F10", 0x44),
        ("F11", 0x57),
        ("F12", 0x58),
        ("Home", 0x47),
        ("ArrowUp", 0x48),
        ("PageUp", 0x49),
        ("ArrowLeft", 0x4B),
        ("ArrowRight", 0x4D),
        ("End", 0x4F),
        ("ArrowDown", 0x50),
        ("PageDown", 0x51),
        ("Insert", 0x52),
        ("Delete", 0x53),
        ("MetaLeft", 0x5B),
    ];
    const LETTERS: &str = "QWERTYUIOPASDFGHJKLZXCVBNM";
    const LETTER_SCANS: [u16; 26] = [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1E, 0x1F, 0x20, 0x21, 0x22,
        0x23, 0x24, 0x25, 0x26, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32,
    ];
    if let Some(letter) = code.strip_prefix("Key").filter(|l| l.len() == 1) {
        let i = LETTERS.find(letter)?;
        return Some((LETTER_SCANS[i], false));
    }
    let extended = matches!(
        code,
        "Home"
            | "End"
            | "PageUp"
            | "PageDown"
            | "Insert"
            | "Delete"
            | "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "MetaLeft"
    );
    NAMED
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, s)| (*s, extended))
}

/// What `release` takes for the left mouse button, beside key codes.
pub const LEFT_BUTTON: &str = "MouseLeft";

/// The points of a drag along `points`: the first, then `steps` evenly spaced points on each
/// segment after it, the segment's end last.
pub fn drag_path(points: &[(f64, f64)], steps: u32) -> Vec<(f64, f64)> {
    let mut path = points.first().copied().into_iter().collect::<Vec<_>>();
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        for n in 1..=steps.max(1) {
            let t = f64::from(n) / f64::from(steps.max(1));
            path.push((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        }
    }
    path
}

/// The keys of a chord in the order a hand presses them: modifiers down, the key down and up,
/// modifiers up in reverse. Each entry is (physical key `code`, key up).
pub fn chord_strokes(code: &str, modifiers: u32) -> Vec<(String, bool)> {
    const MODS: [(u32, &str); 4] = [
        (crate::chord::CTRL, "ControlLeft"),
        (crate::chord::ALT, "AltLeft"),
        (crate::chord::SHIFT, "ShiftLeft"),
        (crate::chord::META, "MetaLeft"),
    ];
    let held: Vec<&str> = MODS
        .iter()
        .filter(|(bit, _)| modifiers & bit != 0)
        .map(|(_, c)| *c)
        .collect();
    let mut out: Vec<(String, bool)> = held.iter().map(|m| (m.to_string(), false)).collect();
    out.push((code.to_string(), false));
    out.push((code.to_string(), true));
    out.extend(held.iter().rev().map(|m| (m.to_string(), true)));
    out
}

/// A point in the page's viewport (CSS pixels) on the screen (physical pixels), given where the
/// viewport's top-left corner is on the screen and the page's device pixel ratio.
pub fn to_screen(origin: (i32, i32), css: (f64, f64), ratio: f64) -> (i32, i32) {
    (
        origin.0 + (css.0 * ratio).round() as i32,
        origin.1 + (css.1 * ratio).round() as i32,
    )
}

#[cfg(windows)]
pub use win::{
    activate, click, drag, is_frontmost, key, release, render_widget, strokes, text,
    top_level_windows, wheel,
};

#[cfg(windows)]
mod win {
    use super::{Activation, Click, OsError};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW,
        GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowPlacement,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow,
        WindowFromPoint, GA_ROOT, GW_OWNER, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE, WINDOWPLACEMENT,
    };

    /// The visible top-level windows of process `pid`, owned ones included, in z-order.
    pub fn top_level_windows(pid: u32) -> Vec<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            // SAFETY: `lparam` is the `(pid, Vec)` passed below, alive for the enumeration.
            let found = unsafe { &mut *(lparam.0 as *mut (u32, Vec<HWND>)) };
            let mut owner_pid = 0u32;
            // SAFETY: `hwnd` comes from EnumWindows; `owner_pid` is a valid out-pointer.
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
                if owner_pid == found.0 && IsWindowVisible(hwnd).as_bool() {
                    found.1.push(hwnd);
                }
            }
            true.into()
        }
        let mut found: (u32, Vec<HWND>) = (pid, Vec::new());
        // SAFETY: the callback reads `found` only while EnumWindows runs.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize)) };
        found.1
    }

    /// The app's main window: of its visible, unowned top-level windows, the largest in its
    /// normal (not minimized) size.
    fn app_window(pid: u32) -> Option<HWND> {
        top_level_windows(pid)
            .into_iter()
            // SAFETY: a plain query on a window handle.
            .filter(|&hwnd| unsafe { GetWindow(hwnd, GW_OWNER) }.is_err())
            .max_by_key(|&hwnd| {
                let mut place = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                // SAFETY: `hwnd` is a window handle; `place` is a valid out-pointer.
                let _ = unsafe { GetWindowPlacement(hwnd, &mut place) };
                let r = place.rcNormalPosition;
                i64::from(r.right - r.left) * i64::from(r.bottom - r.top)
            })
    }

    /// Whether the foreground window belongs to process `pid`.
    pub fn is_frontmost(pid: u32) -> bool {
        front_window(pid).is_ok()
    }

    /// The foreground window, when it belongs to process `pid`.
    fn front_window(pid: u32) -> Result<HWND, OsError> {
        let mut owner = 0u32;
        // SAFETY: plain queries; `owner` is a valid out-pointer.
        let front = unsafe { GetForegroundWindow() };
        // SAFETY: as above.
        unsafe { GetWindowThreadProcessId(front, Some(&mut owner)) };
        if owner == pid {
            Ok(front)
        } else {
            Err(OsError::NotFront(
                "OS input needs the app in front; run `window activate` first. Nothing was sent"
                    .into(),
            ))
        }
    }

    fn wait_frontmost(pid: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while !is_frontmost(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        is_frontmost(pid)
    }

    /// Brings the app's main window to the front, restoring it if minimized: `SetForegroundWindow`
    /// first, then the same while attached to the foreground window's input queue. How it got
    /// there, or `None` when Windows kept it back.
    pub fn activate(pid: u32) -> Result<Option<Activation>, String> {
        if is_frontmost(pid) {
            return Ok(Some(Activation::AlreadyFront));
        }
        let hwnd = app_window(pid).ok_or("the app has no visible window")?;
        // SAFETY: every call takes a live window of the app or plain values.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        if wait_frontmost(pid) {
            return Ok(Some(Activation::Plain));
        }
        // SAFETY: as above; the attachment is undone before returning.
        unsafe {
            let theirs = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let ours = GetCurrentThreadId();
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
        Ok(wait_frontmost(pid).then_some(Activation::AttachInput))
    }

    fn send(inputs: &[INPUT]) -> usize {
        // SAFETY: `inputs` are fully initialised and sized as INPUT.
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize }
    }

    fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        }
    }

    fn physical_key(code: &str, up: bool) -> Result<INPUT, OsError> {
        let (scan, extended) = super::us_scan(code)
            .ok_or_else(|| OsError::Failed(format!("no physical key for `{code}`")))?;
        let mut flags = KEYEVENTF_SCANCODE;
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        Ok(keyboard(0, scan, flags))
    }

    /// Sends physical key strokes, (code, key up), in one `SendInput` call, once the app is in front.
    pub fn strokes(pid: u32, keys: &[(String, bool)]) -> Result<(), OsError> {
        front_window(pid)?;
        let inputs = keys
            .iter()
            .map(|(c, up)| physical_key(c, *up))
            .collect::<Result<Vec<_>, _>>()?;
        let sent = send(&inputs);
        if sent == inputs.len() {
            Ok(())
        } else {
            Err(OsError::Failed(format!(
                "Windows took {sent} of {} key events (an elevated window may be in front)",
                inputs.len()
            )))
        }
    }

    /// Releases keys pokit pressed, whichever app is in front now: a key left down would stay down
    /// for the user.
    pub fn release(codes: &[String]) {
        let inputs: Vec<INPUT> = codes
            .iter()
            .filter_map(|c| {
                if c == super::LEFT_BUTTON {
                    Some(mouse(0, 0, MOUSEEVENTF_LEFTUP, 0))
                } else {
                    physical_key(c, true).ok()
                }
            })
            .collect();
        send(&inputs);
    }

    /// Presses a chord as a hand would, by physical key (its scan code on a US keyboard), so the
    /// user's keyboard layout and input method treat it as they would the real key; one
    /// `SendInput` call, so no other input interleaves. If Windows takes only part of it, every
    /// modifier is released again.
    pub fn key(pid: u32, code: &str, modifiers: u32) -> Result<(), OsError> {
        front_window(pid)?;
        let strokes = super::chord_strokes(code, modifiers);
        let inputs = strokes
            .iter()
            .map(|(c, up)| physical_key(c, *up))
            .collect::<Result<Vec<_>, _>>()?;
        let sent = send(&inputs);
        if sent == inputs.len() {
            return Ok(());
        }
        let release: Vec<INPUT> = strokes
            .iter()
            .filter(|(c, up)| !up && c != code)
            .filter_map(|(c, _)| physical_key(c, true).ok())
            .collect();
        send(&release);
        Err(OsError::Failed(format!(
            "Windows took {sent} of {} key events (an elevated window may be in front); modifiers were released",
            inputs.len()
        )))
    }

    /// Types `text` as Unicode characters, a newline as the Enter key.
    pub fn text(pid: u32, text: &str) -> Result<(), OsError> {
        front_window(pid)?;
        let mut inputs = Vec::new();
        for c in text.chars() {
            if c == '\n' {
                inputs.push(physical_key("Enter", false)?);
                inputs.push(physical_key("Enter", true)?);
                continue;
            }
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                inputs.push(keyboard(0, *unit, KEYEVENTF_UNICODE));
                inputs.push(keyboard(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
        let sent = send(&inputs);
        if sent == inputs.len() {
            Ok(())
        } else {
            Err(OsError::Failed(format!(
                "Windows took {sent} of {} key events (an elevated window may be in front)",
                inputs.len()
            )))
        }
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: `buf` is valid for its length.
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// The WebView2 render widget inside `root`, which holds the page.
    pub fn render_widget(root: HWND) -> Option<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            // SAFETY: `lparam` is the `Option<HWND>` passed below, alive for the enumeration.
            let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
            // SAFETY: `hwnd` comes from the enumeration.
            if class_name(hwnd) == "Chrome_RenderWidgetHostHWND"
                && unsafe { IsWindowVisible(hwnd) }.as_bool()
            {
                *found = Some(hwnd);
                return false.into();
            }
            true.into()
        }
        let mut widget: Option<HWND> = None;
        // SAFETY: the callback writes `widget` only while EnumChildWindows runs.
        let _ = unsafe {
            EnumChildWindows(
                Some(root),
                Some(each),
                LPARAM(&mut widget as *mut _ as isize),
            )
        };
        widget
    }

    fn mouse(x: i32, y: i32, flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
        // SAFETY: plain queries of the virtual screen's bounds.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            )
        };
        let nx = ((i64::from(x - vx) * 65535) / i64::from(vw - 1)) as i32;
        let ny = ((i64::from(y - vy) * 65535) / i64::from(vh - 1)) as i32;
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx,
                    dy: ny,
                    mouseData: data as u32,
                    dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    ..Default::default()
                },
            },
        }
    }

    /// Moves the cursor to a point of the page in the app's front window (CSS pixels) and clicks
    /// there, once the window under that point is that page's; the screen point, in physical
    /// pixels.
    pub fn click(pid: u32, css: (f64, f64), ratio: f64, how: Click) -> Result<(i32, i32), OsError> {
        at_page_point(pid, css, ratio, |at| {
            let mut inputs = vec![mouse(at.0, at.1, MOUSEEVENTF_MOVE, 0)];
            if !how.hover {
                let (down, up) = if how.right {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                };
                for _ in 0..how.count {
                    inputs.push(mouse(at.0, at.1, down, 0));
                    inputs.push(mouse(at.0, at.1, up, 0));
                }
            }
            inputs
        })
    }

    /// Presses the left button at the first point of `path` (page CSS pixels, in the app's front
    /// window, which must be the page there), moves along the rest `step` apart, wherever they
    /// lead on the screen, and releases the button at the last; the last screen point. The
    /// button is released whatever goes wrong after it was pressed.
    pub fn drag(
        pid: u32,
        path: &[(f64, f64)],
        ratio: f64,
        step: std::time::Duration,
    ) -> Result<(i32, i32), OsError> {
        let Some((&first, rest)) = path.split_first() else {
            return Err(OsError::Failed("a drag needs a path".into()));
        };
        let start = at_page_point(pid, first, ratio, |at| {
            vec![
                mouse(at.0, at.1, MOUSEEVENTF_MOVE, 0),
                mouse(at.0, at.1, MOUSEEVENTF_LEFTDOWN, 0),
            ]
        })?;
        /// Releases the left button and puts the thread's DPI context back when dropped, so a
        /// panic mid-drag leaves neither behind.
        struct Held(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
        impl Drop for Held {
            fn drop(&mut self) {
                send(&[mouse(0, 0, MOUSEEVENTF_LEFTUP, 0)]);
                // SAFETY: restores the context this thread had.
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
        // SAFETY: switches this thread to physical pixels; `Held` puts the previous one back.
        let _held = Held(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        });
        let mut at = start;
        let moved = rest.iter().try_for_each(|&(x, y)| {
            std::thread::sleep(step);
            at = (
                start.0 + ((x - first.0) * ratio).round() as i32,
                start.1 + ((y - first.1) * ratio).round() as i32,
            );
            if send(&[mouse(at.0, at.1, MOUSEEVENTF_MOVE, 0)]) == 1 {
                Ok(())
            } else {
                Err(OsError::Failed("Windows did not take a mouse move".into()))
            }
        });
        std::thread::sleep(step);
        moved.map(|_| at)
    }

    /// Moves the cursor to a point of the page and turns the wheel `notches` notches, positive
    /// towards the user (scrolling down), as a hand would; the screen point.
    pub fn wheel(
        pid: u32,
        css: (f64, f64),
        ratio: f64,
        notches: i32,
    ) -> Result<(i32, i32), OsError> {
        at_page_point(pid, css, ratio, |at| {
            let mut inputs = vec![mouse(at.0, at.1, MOUSEEVENTF_MOVE, 0)];
            for _ in 0..notches.unsigned_abs() {
                let delta = if notches > 0 {
                    -WHEEL_DELTA
                } else {
                    WHEEL_DELTA
                };
                inputs.push(mouse(at.0, at.1, MOUSEEVENTF_WHEEL, delta));
            }
            inputs
        })
    }

    /// One notch of the wheel, in the units `MOUSEEVENTF_WHEEL` takes (winuser.h `WHEEL_DELTA`).
    const WHEEL_DELTA: i32 = 120;

    /// Places a page point (CSS pixels) on the screen in the app's front window, checks that the
    /// page is what is there, and sends the mouse input `inputs_at` builds for that point.
    fn at_page_point(
        pid: u32,
        css: (f64, f64),
        ratio: f64,
        inputs_at: impl FnOnce((i32, i32)) -> Vec<INPUT>,
    ) -> Result<(i32, i32), OsError> {
        // SAFETY: switches this thread to physical pixels; the previous context is put back below.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let result = (|| {
            let root = front_window(pid)?;
            let widget = render_widget(root).ok_or_else(|| {
                OsError::NotFront(
                    "the app's front window holds no page (a dialog?); nothing was sent".into(),
                )
            })?;
            let mut corner = POINT { x: 0, y: 0 };
            // SAFETY: `widget` is a live window; `corner` is a valid in-out point.
            if !unsafe { ClientToScreen(widget, &mut corner) }.as_bool() {
                return Err(OsError::Failed(
                    "could not place the page on the screen".into(),
                ));
            }
            let at = super::to_screen((corner.x, corner.y), css, ratio);
            // SAFETY: plain queries on a point and the window found there.
            let under =
                unsafe { GetAncestor(WindowFromPoint(POINT { x: at.0, y: at.1 }), GA_ROOT) };
            if under != root {
                return Err(OsError::NotFront(format!(
                    "something other than the page is at ({}, {}) on the screen; nothing was sent",
                    at.0, at.1
                )));
            }
            let inputs = inputs_at(at);
            let sent = send(&inputs);
            if sent == inputs.len() {
                Ok(at)
            } else {
                Err(OsError::Failed(format!(
                    "Windows took {sent} of {} mouse events",
                    inputs.len()
                )))
            }
        })();
        // SAFETY: restores the context this thread had.
        unsafe { SetThreadDpiAwarenessContext(previous) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_path_steps_evenly_along_each_part_and_ends_on_each_point() {
        let path = drag_path(&[(0.0, 0.0), (10.0, 0.0), (10.0, 20.0)], 2);
        assert_eq!(
            path,
            vec![
                (0.0, 0.0),
                (5.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (10.0, 20.0)
            ]
        );
    }

    #[test]
    fn a_chord_is_pressed_modifiers_first_and_released_in_reverse() {
        let strokes = chord_strokes("KeyK", crate::chord::CTRL | crate::chord::SHIFT);
        let expected = [
            ("ControlLeft", false),
            ("ShiftLeft", false),
            ("KeyK", false),
            ("KeyK", true),
            ("ShiftLeft", true),
            ("ControlLeft", true),
        ];
        assert_eq!(strokes.len(), expected.len());
        for ((code, up), (want, want_up)) in strokes.iter().zip(expected) {
            assert_eq!((code.as_str(), *up), (want, want_up));
        }
    }

    #[test]
    fn every_key_the_chord_table_knows_has_a_physical_key() {
        for code in [
            "KeyA",
            "KeyQ",
            "KeyZ",
            "Digit0",
            "Digit9",
            "Space",
            "Minus",
            "Equal",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Semicolon",
            "Quote",
            "Backquote",
            "Comma",
            "Period",
            "Slash",
            "Enter",
            "Escape",
            "Tab",
            "Backspace",
            "Delete",
            "Insert",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "F1",
            "F12",
        ] {
            assert!(us_scan(code).is_some(), "{code}");
        }
        assert_eq!(us_scan("KeyA"), Some((0x1E, false)));
        assert_eq!(us_scan("KeyQ"), Some((0x10, false)));
        assert_eq!(us_scan("ArrowLeft"), Some((0x4B, true)));
        assert_eq!(us_scan("Enter"), Some((0x1C, false)));
        assert_eq!(us_scan("Nothing"), None);
    }

    #[test]
    fn a_page_point_scales_by_the_pixel_ratio_from_the_viewport_corner() {
        assert_eq!(to_screen((100, 200), (10.0, 20.0), 1.5), (115, 230));
        assert_eq!(to_screen((0, 0), (33.4, 0.0), 1.0), (33, 0));
    }
}

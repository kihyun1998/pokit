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

/// A mouse button and how many presses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub right: bool,
    pub count: u32,
    pub hover: bool,
}

/// The keys of a chord in the order a hand presses them: modifiers down, the key down and up,
/// modifiers up in reverse. Each entry is (virtual key, key up).
pub fn chord_strokes(vk: u16, modifiers: u32) -> Vec<(u16, bool)> {
    const MODS: [(u32, u16); 4] = [
        (crate::chord::CTRL, 0x11),
        (crate::chord::ALT, 0x12),
        (crate::chord::SHIFT, 0x10),
        (crate::chord::META, 0x5B),
    ];
    let held: Vec<u16> = MODS
        .iter()
        .filter(|(bit, _)| modifiers & bit != 0)
        .map(|(_, vk)| *vk)
        .collect();
    let mut out: Vec<(u16, bool)> = held.iter().map(|m| (*m, false)).collect();
    out.push((vk, false));
    out.push((vk, true));
    out.extend(held.iter().rev().map(|m| (*m, true)));
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
pub use win::{activate, click, is_frontmost, key, text};

#[cfg(windows)]
mod win {
    use super::{Activation, Click};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
        KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumChildWindows, EnumWindows, GetClassNameW, GetForegroundWindow,
        GetSystemMetrics, GetWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
    };

    /// The app's main window: its largest visible, unowned top-level window.
    fn app_window(pid: u32) -> Option<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            // SAFETY: `lparam` is the `(pid, Vec)` passed below, alive for the enumeration.
            let found = unsafe { &mut *(lparam.0 as *mut (u32, Vec<(HWND, i64)>)) };
            let mut owner_pid = 0u32;
            // SAFETY: `hwnd` comes from EnumWindows; every out-pointer is a valid local.
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
                if owner_pid == found.0
                    && IsWindowVisible(hwnd).as_bool()
                    && GetWindow(hwnd, GW_OWNER).is_err()
                {
                    let mut r = Default::default();
                    let _ = GetWindowRect(hwnd, &mut r);
                    let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
                    found.1.push((hwnd, area));
                }
            }
            true.into()
        }
        let mut found: (u32, Vec<(HWND, i64)>) = (pid, Vec::new());
        // SAFETY: the callback reads `found` only while EnumWindows runs.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize)) };
        found
            .1
            .into_iter()
            .max_by_key(|(_, area)| *area)
            .map(|(h, _)| h)
    }

    /// Whether the foreground window belongs to process `pid`.
    pub fn is_frontmost(pid: u32) -> bool {
        let mut owner = 0u32;
        // SAFETY: plain queries; `owner` is a valid out-pointer.
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut owner)) };
        owner == pid
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

    /// Refuses unless the app is in front, so OS input can never land in another app.
    fn guard(pid: u32) -> Result<(), String> {
        if is_frontmost(pid) {
            Ok(())
        } else {
            Err(
                "OS input needs the app in front; run `window activate` first. Nothing was sent"
                    .into(),
            )
        }
    }

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        // SAFETY: `inputs` are fully initialised and sized as INPUT.
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(format!(
                "Windows took {sent} of {} input events (another program's elevated window may be in front)",
                inputs.len()
            ))
        }
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

    /// Whether a virtual key is one of the extended keys (arrows and the navigation block).
    fn extended(vk: u16) -> bool {
        matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C)
    }

    /// Presses a chord as a hand would, each key with its scan code (the page's `code` comes from
    /// it); one `SendInput` call, so no other input interleaves.
    pub fn key(pid: u32, vk: u16, modifiers: u32) -> Result<(), String> {
        guard(pid)?;
        let inputs: Vec<INPUT> = super::chord_strokes(vk, modifiers)
            .into_iter()
            .map(|(vk, up)| {
                let mut flags = KEYBD_EVENT_FLAGS(0);
                if up {
                    flags |= KEYEVENTF_KEYUP;
                }
                if extended(vk) {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                // SAFETY: a plain lookup in the active keyboard layout.
                let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
                keyboard(vk, scan, flags)
            })
            .collect();
        send(&inputs)
    }

    /// Types `text` as Unicode characters, a newline as Enter.
    pub fn text(pid: u32, text: &str) -> Result<(), String> {
        guard(pid)?;
        let mut inputs = Vec::new();
        for c in text.chars() {
            if c == '\n' {
                inputs.push(keyboard(0x0D, 0, KEYBD_EVENT_FLAGS(0)));
                inputs.push(keyboard(0x0D, 0, KEYEVENTF_KEYUP));
                continue;
            }
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                inputs.push(keyboard(0, *unit, KEYEVENTF_UNICODE));
                inputs.push(keyboard(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
        send(&inputs)
    }

    /// The screen position (physical pixels) of the top-left corner of the app's page: the
    /// client area of the WebView2 render widget inside the app's main window.
    fn viewport_origin(pid: u32) -> Result<(i32, i32), String> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            // SAFETY: `lparam` is the `Option<HWND>` passed below, alive for the enumeration.
            let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
            let mut buf = [0u16; 64];
            // SAFETY: `buf` is valid for its length.
            let n = unsafe { GetClassNameW(hwnd, &mut buf) };
            // SAFETY: as above.
            if String::from_utf16_lossy(&buf[..n.max(0) as usize]) == "Chrome_RenderWidgetHostHWND"
                && unsafe { IsWindowVisible(hwnd) }.as_bool()
            {
                *found = Some(hwnd);
                return false.into();
            }
            true.into()
        }
        let main = app_window(pid).ok_or("the app has no visible window")?;
        let mut widget: Option<HWND> = None;
        // SAFETY: the callback writes `widget` only while EnumChildWindows runs.
        let _ = unsafe {
            EnumChildWindows(
                Some(main),
                Some(each),
                LPARAM(&mut widget as *mut _ as isize),
            )
        };
        let widget = widget.ok_or("the app's window holds no WebView2 page")?;
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: `widget` is a live window; `p` is a valid in-out point.
        if !unsafe { ClientToScreen(widget, &mut p) }.as_bool() {
            return Err("could not place the page on the screen".into());
        }
        Ok((p.x, p.y))
    }

    fn mouse(x: i32, y: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        // SAFETY: plain queries of the virtual screen's bounds.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            )
        };
        let nx = ((i64::from(x - vx) * 65535) / i64::from(vw - 1).max(1)) as i32;
        let ny = ((i64::from(y - vy) * 65535) / i64::from(vh - 1).max(1)) as i32;
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx,
                    dy: ny,
                    dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    ..Default::default()
                },
            },
        }
    }

    /// Moves the cursor to a point of the page (CSS pixels) and clicks there; the screen point it
    /// used, in physical pixels.
    pub fn click(pid: u32, css: (f64, f64), ratio: f64, how: Click) -> Result<(i32, i32), String> {
        // SAFETY: switches this thread to physical pixels; the previous context is put back.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let result = (|| {
            guard(pid)?;
            let at = super::to_screen(viewport_origin(pid)?, css, ratio);
            let mut inputs = vec![mouse(at.0, at.1, MOUSEEVENTF_MOVE)];
            if !how.hover {
                let (down, up) = if how.right {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                };
                for _ in 0..how.count {
                    inputs.push(mouse(at.0, at.1, down));
                    inputs.push(mouse(at.0, at.1, up));
                }
            }
            send(&inputs).map(|_| at)
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
    fn a_chord_is_pressed_modifiers_first_and_released_in_reverse() {
        let ctrl_shift = crate::chord::CTRL | crate::chord::SHIFT;
        assert_eq!(
            chord_strokes(0x4B, ctrl_shift),
            vec![
                (0x11, false),
                (0x10, false),
                (0x4B, false),
                (0x4B, true),
                (0x10, true),
                (0x11, true)
            ]
        );
        assert_eq!(chord_strokes(0x0D, 0), vec![(0x0D, false), (0x0D, true)]);
    }

    #[test]
    fn a_page_point_scales_by_the_pixel_ratio_from_the_viewport_corner() {
        assert_eq!(to_screen((100, 200), (10.0, 20.0), 1.5), (115, 230));
        assert_eq!(to_screen((0, 0), (33.4, 0.0), 1.0), (33, 0));
    }
}

//! Win32 windows from a test's side: the foreground window, an app's windows, minimizing and
//! restoring without activating, and a topmost window that covers another without taking focus.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Hwnd = *mut c_void;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> Hwnd;
    fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    fn EnumWindows(callback: extern "system" fn(Hwnd, isize) -> i32, lparam: isize) -> i32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn WindowFromPoint(x: i32, y: i32) -> Hwnd;
    fn GetAncestor(hwnd: Hwnd, flags: u32) -> Hwnd;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
}

const SW_SHOWNOACTIVATE: i32 = 4;
const SW_SHOWMINNOACTIVE: i32 = 7;
const WS_POPUP: u32 = 0x8000_0000;
const SS_NOTIFY: u32 = 0x0000_0100;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const PM_REMOVE: u32 = 1;

/// The foreground window, as a number (0 when there is none).
pub fn foreground() -> isize {
    // SAFETY: takes no arguments and only returns a handle.
    unsafe { GetForegroundWindow() as isize }
}

/// The process that owns `hwnd`.
pub fn window_pid(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out-pointer; an invalid handle leaves it 0.
    unsafe { GetWindowThreadProcessId(hwnd as Hwnd, &mut pid) };
    pid
}

/// The visible top-level windows of process `pid`.
pub fn windows_of(pid: u32) -> Vec<isize> {
    struct Found {
        pid: u32,
        windows: Vec<isize>,
    }
    extern "system" fn each(hwnd: Hwnd, lparam: isize) -> i32 {
        // SAFETY: `lparam` is the `Found` passed to `EnumWindows` below, alive for the call.
        let found = unsafe { &mut *(lparam as *mut Found) };
        // SAFETY: `hwnd` comes from `EnumWindows`.
        if window_pid(hwnd as isize) == found.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            found.windows.push(hwnd as isize);
        }
        1
    }
    let mut found = Found {
        pid,
        windows: Vec::new(),
    };
    // SAFETY: the callback only reads `found` through `lparam` while `EnumWindows` runs.
    unsafe { EnumWindows(each, &mut found as *mut Found as isize) };
    found.windows
}

pub fn rect(hwnd: isize) -> Rect {
    let mut r = Rect::default();
    // SAFETY: `r` is a valid out-pointer.
    unsafe { GetWindowRect(hwnd as Hwnd, &mut r) };
    r
}

/// Minimizes `hwnd` without activating another window.
pub fn minimize(hwnd: isize) {
    // SAFETY: an invalid handle makes this a no-op.
    unsafe { ShowWindow(hwnd as Hwnd, SW_SHOWMINNOACTIVE) };
}

/// Restores `hwnd` to its last size and place without activating it.
pub fn restore(hwnd: isize) {
    // SAFETY: an invalid handle makes this a no-op.
    unsafe { ShowWindow(hwnd as Hwnd, SW_SHOWNOACTIVATE) };
}

/// An opaque topmost window over a rectangle that never takes focus; removed on drop.
pub struct Cover {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Cover {
    /// Covers `r` with a margin of `margin` pixels on every side.
    pub fn over(r: Rect, margin: i32) -> Cover {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: `class` is a NUL-terminated predefined class name; the window is owned and
            // destroyed by this thread, which also pumps its messages.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP | WS_VISIBLE | SS_NOTIFY,
                    r.left - margin,
                    r.top - margin,
                    r.right - r.left + 2 * margin,
                    r.bottom - r.top + 2 * margin,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            let _ = ready_tx.send(hwnd as isize);
            if hwnd.is_null() {
                return;
            }
            let mut msg: Msg = unsafe { std::mem::zeroed() };
            while !flag.load(Ordering::SeqCst) {
                // SAFETY: `msg` is a valid out-pointer for this thread's queue.
                while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
                {
                    // SAFETY: `msg` was filled by `PeekMessageW`.
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            // SAFETY: `hwnd` was created by this thread.
            unsafe { DestroyWindow(hwnd) };
        });
        let hwnd = ready_rx.recv().unwrap();
        assert_ne!(hwnd, 0, "the covering window was not created");
        Cover {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Cover {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Whether `hwnd` is minimized.
pub fn is_minimized(hwnd: isize) -> bool {
    // SAFETY: an invalid handle reads as not minimized.
    unsafe { IsIconic(hwnd as Hwnd) != 0 }
}

/// The window's title.
pub fn title(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is valid for its length.
    let n = unsafe { GetWindowTextW(hwnd as Hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The top-level window under the screen point (`x`, `y`).
pub fn top_level_at(x: i32, y: i32) -> isize {
    const GA_ROOT: u32 = 2;
    // SAFETY: both calls take plain values and return handles.
    unsafe { GetAncestor(WindowFromPoint(x, y), GA_ROOT) as isize }
}

/// The process ids from `pid` up through its parents, read with PowerShell.
pub fn ancestors(pid: u32) -> Vec<u32> {
    let script = format!(
        "$p = {pid}; $ids = @(); for ($i = 0; $i -lt 32 -and $p; $i++) {{ $ids += $p; $c = Get-CimInstance Win32_Process -Filter \"ProcessId=$p\"; if (-not $c) {{ break }}; $p = $c.ParentProcessId }}; $ids -join ' '"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .expect("powershell did not start");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Whether this test descends from the foreground app, so that a launch could take the
/// foreground at all; says why not when it does not.
pub fn can_take_foreground(test: &str) -> bool {
    let owner = window_pid(foreground());
    let can = ancestors(std::process::id()).contains(&owner);
    if !can {
        eprintln!(
            "{test}: the foreground app (pid {owner}) did not start this test, so launch could not take the foreground either way; run it from the terminal in front to test anything"
        );
    }
    can
}

/// The system's double-click time: clicks closer together than this are one sequence.
pub fn double_click_time() -> std::time::Duration {
    #[link(name = "user32")]
    extern "system" {
        fn GetDoubleClickTime() -> u32;
    }
    // SAFETY: takes no arguments.
    std::time::Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }))
}

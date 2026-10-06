//! The Windows clipboard: reading and writing text, and saving and restoring whatever it held.

/// One clipboard format and its bytes.
#[cfg_attr(not(windows), allow(dead_code))]
pub type Item = (u32, Vec<u8>);

/// Whether a format's data is plain memory that can be copied out and put back. Bitmaps,
/// metafiles, palettes and owner-display formats are handles of other kinds; private and
/// GDI-object formats are the owner's own handles.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn restorable(format: u32) -> bool {
    !matches!(format, 2 | 3 | 9 | 14 | 0x80..=0x8F | 0x200..=0x3FF)
}

/// UTF-16 with a terminating NUL, as `CF_UNICODETEXT` holds it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The text in `CF_UNICODETEXT` bytes, up to the first NUL.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn text_from_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(windows)]
pub use win::Clipboard;

#[cfg(windows)]
mod win {
    use super::Item;
    use core::ffi::c_void;
    use std::sync::mpsc;
    use std::time::Duration;

    type Hwnd = *mut c_void;
    type Handle = *mut c_void;

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
        fn OpenClipboard(owner: Hwnd) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(format: u32) -> Handle;
        fn SetClipboardData(format: u32, data: Handle) -> Handle;
        fn EnumClipboardFormats(format: u32) -> u32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn GetClipboardSequenceNumber() -> u32;
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
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn PostThreadMessageW(thread: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
        fn GlobalLock(mem: Handle) -> *mut c_void;
        fn GlobalUnlock(mem: Handle) -> i32;
        fn GlobalSize(mem: Handle) -> usize;
        fn GlobalFree(mem: Handle) -> Handle;
        fn GetCurrentThreadId() -> u32;
    }

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const HWND_MESSAGE: isize = -3;
    const WM_APP: u32 = 0x8000;
    const PM_NOREMOVE: u32 = 0;
    const EXCLUDE: &str = "ExcludeClipboardContentFromMonitorProcessing";

    type Job = Box<dyn FnOnce(Hwnd, &mut Saved) + Send>;

    /// What the clipboard held before pokit's first write, and its sequence number right after
    /// pokit's last write. Kept on the clipboard thread, so jobs read and change it in order.
    #[derive(Default)]
    struct Saved {
        before: Option<Vec<Item>>,
        written: Option<u32>,
    }

    /// A message-only window that owns what pokit puts on the clipboard, on a thread of its own
    /// that keeps pumping messages; every clipboard operation runs on that thread, one at a time.
    pub struct Clipboard {
        jobs: mpsc::Sender<Job>,
        thread: u32,
    }

    impl Clipboard {
        pub fn start() -> Result<Clipboard, String> {
            let (jobs, rx) = mpsc::channel::<Job>();
            let (ready_tx, ready_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
                // SAFETY: `class` is a NUL-terminated predefined class; the window lives as long
                // as this thread, which pumps its messages.
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        std::ptr::null(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        HWND_MESSAGE as Hwnd,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                let mut msg: Msg = unsafe { std::mem::zeroed() };
                // SAFETY: creates this thread's message queue before its id is handed out.
                unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
                // SAFETY: takes no arguments.
                let thread = unsafe { GetCurrentThreadId() };
                let _ = ready_tx.send((!hwnd.is_null()).then_some(thread));
                if hwnd.is_null() {
                    return;
                }
                let mut saved = Saved::default();
                // SAFETY: `msg` is a valid out-pointer; the loop ends when the thread's queue is
                // quit or fails.
                while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                    if msg.message == WM_APP {
                        while let Ok(job) = rx.try_recv() {
                            job(hwnd, &mut saved);
                        }
                    } else {
                        // SAFETY: `msg` was filled by `GetMessageW`.
                        unsafe { DispatchMessageW(&msg) };
                    }
                }
            });
            match ready_rx.recv() {
                Ok(Some(thread)) => Ok(Clipboard { jobs, thread }),
                _ => Err("could not create the clipboard owner window".into()),
            }
        }

        /// Runs `f` with the owner window on the clipboard thread and returns its result.
        fn run<T: Send + 'static>(
            &self,
            f: impl FnOnce(Hwnd, &mut Saved) -> T + Send + 'static,
        ) -> Result<T, String> {
            let (tx, rx) = mpsc::channel();
            self.jobs
                .send(Box::new(move |hwnd, saved: &mut Saved| {
                    let _ = tx.send(f(hwnd, saved));
                }))
                .map_err(|_| "the clipboard thread has ended".to_string())?;
            // SAFETY: posts to a thread id this process created.
            unsafe { PostThreadMessageW(self.thread, WM_APP, 0, 0) };
            rx.recv_timeout(Duration::from_secs(10))
                .map_err(|_| "the clipboard did not answer within 10s".to_string())
        }

        /// The clipboard's text, if it holds any.
        pub fn read_text(&self) -> Result<Option<String>, String> {
            self.run(|hwnd, _| {
                let _open = Open::new(hwnd)?;
                // SAFETY: the clipboard is open.
                if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
                    return Ok(None);
                }
                Ok(read_format(CF_UNICODETEXT).map(|b| super::text_from_utf16(&b)))
            })?
        }

        /// Puts `text` on the clipboard, first saving what it held if this is pokit's first
        /// write; true when it saved. A write that fails after emptying the clipboard still
        /// counts as pokit's, so the saved clipboard comes back at the end.
        pub fn write_text(&self, text: &str) -> Result<bool, String> {
            let items = vec![(CF_UNICODETEXT, super::utf16_bytes(text))];
            self.run(move |hwnd, saved| {
                let saving = saved.before.is_none();
                if saving {
                    saved.before = Some(snapshot(hwnd)?);
                }
                let (touched, result) = put(hwnd, &items);
                if touched {
                    saved.written = Some(sequence());
                }
                result.map(|_| saving)
            })?
        }

        /// Puts back what the clipboard held before pokit's first write, unless something else
        /// has written to it since; true when it did. Formats that cannot be put back are
        /// skipped and named in the error, after the rest are restored.
        pub fn restore(&self) -> Result<bool, String> {
            self.run(|hwnd, saved| {
                let (Some(before), Some(written)) = (saved.before.take(), saved.written.take())
                else {
                    return Ok(false);
                };
                if sequence() != written {
                    return Ok(false);
                }
                put(hwnd, &before).1.map(|_| true)
            })?
        }
    }

    /// Every restorable format on the clipboard, with its bytes.
    fn snapshot(hwnd: Hwnd) -> Result<Vec<Item>, String> {
        let _open = Open::new(hwnd)?;
        let mut items = Vec::new();
        let mut format = 0;
        loop {
            // SAFETY: the clipboard is open.
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if super::restorable(format) {
                if let Some(bytes) = read_format(format) {
                    items.push((format, bytes));
                }
            }
        }
        Ok(items)
    }

    /// Replaces the clipboard with `items`, kept out of clipboard history and cloud sync.
    /// Returns whether the clipboard was emptied, and whether every format went on.
    fn put(hwnd: Hwnd, items: &[Item]) -> (bool, Result<(), String>) {
        let _open = match Open::new(hwnd) {
            Ok(open) => open,
            Err(e) => return (false, Err(e)),
        };
        // SAFETY: the clipboard is open with `hwnd` as its owner-to-be.
        if unsafe { EmptyClipboard() } == 0 {
            return (false, Err("could not empty the clipboard".to_string()));
        }
        let mut failed = Vec::new();
        if !items.is_empty() {
            let name: Vec<u16> = EXCLUDE.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `name` is NUL-terminated.
            let exclude = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
            if let Err(e) = write_format(exclude, &0u32.to_le_bytes()) {
                failed.push(e);
            }
        }
        for (format, bytes) in items {
            if let Err(e) = write_format(*format, bytes) {
                failed.push(e);
            }
        }
        (
            true,
            if failed.is_empty() {
                Ok(())
            } else {
                Err(failed.join("; "))
            },
        )
    }

    /// The clipboard's sequence number, which changes whenever its contents do.
    fn sequence() -> u32 {
        // SAFETY: takes no arguments.
        unsafe { GetClipboardSequenceNumber() }
    }

    /// The clipboard, open until dropped.
    struct Open;

    impl Open {
        /// Opens the clipboard for `owner`, retrying for up to 1 s while another window holds it.
        fn new(owner: Hwnd) -> Result<Open, String> {
            for _ in 0..100 {
                // SAFETY: `owner` is this thread's window.
                if unsafe { OpenClipboard(owner) } != 0 {
                    return Ok(Open);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err("another program kept the clipboard open for 1s".into())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: the clipboard was opened by `Open::new` on this thread.
            unsafe { CloseClipboard() };
        }
    }

    /// The bytes of `format`, while the clipboard is open.
    fn read_format(format: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open; the handle is the clipboard's and is only read while
        // locked.
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            let ptr = GlobalLock(handle) as *const u8;
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }

    /// Puts `bytes` on the clipboard as `format`, while it is open and owned.
    fn write_format(format: u32, bytes: &[u8]) -> Result<(), String> {
        // SAFETY: the allocation is filled while locked; on success the clipboard owns it, and
        // on failure it is freed here.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
            if mem.is_null() {
                return Err("could not allocate clipboard memory".into());
            }
            let ptr = GlobalLock(mem) as *mut u8;
            if ptr.is_null() {
                GlobalFree(mem);
                return Err("could not lock clipboard memory".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            GlobalUnlock(mem);
            if SetClipboardData(format, mem).is_null() {
                GlobalFree(mem);
                return Err(format!("could not put format {format} on the clipboard"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_that_are_not_plain_memory_are_not_restored() {
        for format in [2, 3, 9, 14, 0x80, 0x82, 0x8E, 0x200, 0x2FF, 0x300, 0x3FF] {
            assert!(!restorable(format), "{format:#x}");
        }
        for format in [1, 7, 8, 13, 15, 16, 17, 0xC000, 0xC123] {
            assert!(restorable(format), "{format:#x}");
        }
    }

    #[test]
    fn text_survives_the_round_trip_through_utf16_and_stops_at_nul() {
        let bytes = utf16_bytes("한글 ✓ text");
        assert_eq!(bytes.len() % 2, 0);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
        assert_eq!(text_from_utf16(&bytes), "한글 ✓ text");
        let mut padded = bytes.clone();
        padded.extend([b'x', 0, b'y', 0]);
        assert_eq!(text_from_utf16(&padded), "한글 ✓ text");
    }
}

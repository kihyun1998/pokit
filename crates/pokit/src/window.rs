//! The app's windows: which one holds a page, and moving and resizing it without activating it.

/// Where a page is on the screen and how big, in CSS pixels, as it reports itself
/// (`screenX`, `screenY`, `innerWidth`, `innerHeight`, `devicePixelRatio`).
#[derive(Clone, Copy, Debug)]
pub struct PageBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub ratio: f64,
}

/// A window's outer rectangle in its own logical pixels (physical pixels over its scale), and
/// that scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// A window's page area: its origin and size on the screen in physical pixels, and the window's
/// scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

/// Which of `areas` holds `page`: the one within a CSS pixel of it on every side. The size is
/// compared at the page's `devicePixelRatio`; the origin at that or at the window's scale, which
/// differ when the page is zoomed.
pub fn matching(page: PageBox, areas: &[Area]) -> Result<usize, String> {
    let near =
        |physical: i32, css: f64, ratio: f64| (f64::from(physical) / ratio - css).abs() <= 1.0;
    let origin = |a: &Area, ratio: f64| near(a.x, page.x, ratio) && near(a.y, page.y, ratio);
    let found: Vec<usize> = areas
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            (origin(a, page.ratio) || origin(a, a.scale))
                && near(a.width, page.width, page.ratio)
                && near(a.height, page.height, page.ratio)
        })
        .map(|(i, _)| i)
        .collect();
    match found[..] {
        [one] => Ok(one),
        [] => Err("no window of the app holds this page".into()),
        _ => Err(format!(
            "{} of the app's windows sit exactly where this page is; cannot tell which holds it",
            found.len()
        )),
    }
}

/// The outer size, in the window's logical pixels, that gives the page a viewport of `want`
/// CSS pixels, from the window's outer size now (`placed`) and its page area now (`area`).
pub fn outer_for_viewport(want: (f64, f64), ratio: f64, placed: Placed, area: Area) -> (f64, f64) {
    let grow = |want: f64, now: i32| (want * ratio - f64::from(now)) / placed.scale;
    (
        placed.width + grow(want.0, area.width),
        placed.height + grow(want.1, area.height),
    )
}

/// A capture of a window with the app's own windows over it: RGBA pixels, row by row, and the
/// windows drawn, bottom first.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub windows: Vec<Drawn>,
}

/// One window drawn into a capture: its title, class, and the part of the capture it covers
/// (x, y, width, height in the capture's pixels).
pub struct Drawn {
    pub title: String,
    pub class: String,
    pub rect: (i32, i32, i32, i32),
}

/// `shot` as a PNG file's bytes.
pub fn png(shot: &Shot) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, shot.width, shot.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&shot.rgba))
        .map_err(|e| format!("could not encode the capture: {e}"))?;
    Ok(out)
}

/// Why no window was found for a page.
#[derive(Debug)]
pub enum Unplaced {
    /// None of the app's visible windows holds the page, and one of its windows is minimized.
    Minimized,
    NotFound(String),
}

#[cfg(windows)]
pub use win::{area, capture, page_window, placed, set};

#[cfg(windows)]
mod win {
    use super::{Area, Drawn, PageBox, Placed, Shot, Unplaced};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
    };
    use windows::Win32::Graphics::Gdi::{
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HDC,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::HiDpi::{
        GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetClientRect, GetWindowRect, GetWindowTextW, IsIconic, IsZoomed,
        SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    };

    /// Runs `f` with this thread in physical pixels, and puts the previous context back.
    fn physical<T>(f: impl FnOnce() -> T) -> T {
        // SAFETY: switches this thread's DPI context; the previous one is put back below.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let out = f();
        // SAFETY: restores the context this thread had.
        unsafe { SetThreadDpiAwarenessContext(previous) };
        out
    }

    fn handle(hwnd: isize) -> HWND {
        HWND(hwnd as *mut _)
    }

    fn scale(hwnd: HWND) -> Result<f64, String> {
        // SAFETY: a plain query on a window handle.
        match unsafe { GetDpiForWindow(hwnd) } {
            0 => Err("the window is gone".into()),
            dpi => Ok(f64::from(dpi) / 96.0),
        }
    }

    /// The app's top-level window that holds `page`, found by where its page area is on the
    /// screen.
    pub fn page_window(pid: u32, page: PageBox) -> Result<isize, Unplaced> {
        physical(|| {
            let mut windows = Vec::new();
            let mut areas = Vec::new();
            let mut minimized = false;
            for root in crate::os_input::top_level_windows(pid) {
                // SAFETY: a plain query on a window handle.
                if unsafe { IsIconic(root) }.as_bool() {
                    minimized = true;
                } else if let Some(a) = page_area(root) {
                    windows.push(root);
                    areas.push(a);
                }
            }
            match super::matching(page, &areas) {
                Ok(i) => Ok(windows[i].0 as isize),
                Err(_) if minimized => Err(Unplaced::Minimized),
                Err(e) => Err(Unplaced::NotFound(e)),
            }
        })
    }

    fn page_area(root: HWND) -> Option<Area> {
        let widget = crate::os_input::render_widget(root)?;
        let mut corner = POINT { x: 0, y: 0 };
        let mut client = RECT::default();
        // SAFETY: `widget` is a live window; both out-pointers are valid locals.
        let ok = unsafe {
            ClientToScreen(widget, &mut corner).as_bool()
                && GetClientRect(widget, &mut client).is_ok()
        };
        Some(Area {
            x: corner.x,
            y: corner.y,
            width: client.right,
            height: client.bottom,
            scale: scale(root).ok()?,
        })
        .filter(|_| ok)
    }

    /// Window `hwnd`'s page area now.
    pub fn area(hwnd: isize) -> Option<Area> {
        physical(|| page_area(handle(hwnd)))
    }

    /// Window `hwnd`'s outer rectangle now, in its logical pixels.
    pub fn placed(hwnd: isize) -> Result<Placed, String> {
        physical(|| {
            let hwnd = handle(hwnd);
            let scale = scale(hwnd)?;
            let mut r = RECT::default();
            // SAFETY: a plain query; `r` is a valid out-pointer.
            unsafe { GetWindowRect(hwnd, &mut r) }.map_err(|e| e.to_string())?;
            let logical = |v: i32| (f64::from(v) / scale * 100.0).round() / 100.0;
            Ok(Placed {
                x: logical(r.left),
                y: logical(r.top),
                width: logical(r.right - r.left),
                height: logical(r.bottom - r.top),
                scale,
            })
        })
    }

    /// What the user sees of a window: its frame without Windows' invisible resize borders.
    fn frame(hwnd: HWND) -> RECT {
        let mut r = RECT::default();
        // SAFETY: `r` is a valid out-pointer of the size passed.
        let dwm = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut r as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if dwm.is_err() {
            // SAFETY: a plain query; `r` is a valid out-pointer.
            let _ = unsafe { GetWindowRect(hwnd, &mut r) };
        }
        r
    }

    /// `PW_RENDERFULLCONTENT`: draw content composed outside the window's own drawing too.
    const RENDER_FULL_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

    /// Whether DWM keeps the window from being seen (on another virtual desktop, for one).
    fn cloaked(hwnd: HWND) -> bool {
        let mut cloak = 0u32;
        // SAFETY: `cloak` is a valid out-pointer of the size passed.
        let read = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloak as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            )
        };
        read.is_ok() && cloak != 0
    }

    fn text(hwnd: HWND, class: bool) -> String {
        let mut buf = [0u16; 256];
        // SAFETY: `buf` is valid for its length.
        let n = unsafe {
            if class {
                GetClassNameW(hwnd, &mut buf)
            } else {
                GetWindowTextW(hwnd, &mut buf)
            }
        };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// Window `hwnd` as it draws itself, whatever covers it (`PrintWindow` with
    /// `PW_RENDERFULLCONTENT`): its window rectangle and BGRA pixels.
    fn print(screen: HDC, hwnd: HWND) -> Option<(RECT, Vec<u8>)> {
        let mut r = RECT::default();
        // SAFETY: a plain query; `r` is a valid out-pointer.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        // SAFETY: GDI objects are created here, used only here, and deleted before returning.
        unsafe {
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bitmap.into());
            let printed = PrintWindow(hwnd, dc, RENDER_FULL_CONTENT).as_bool();
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bgra = vec![0u8; (w * h * 4) as usize];
            SelectObject(dc, old);
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(bgra.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            (printed && lines == h).then_some((r, bgra))
        }
    }

    /// The window holding a page, with the app's own windows over it drawn in, each as it draws
    /// itself: windows of other apps never get in, whether they cover it or not.
    pub fn capture(pid: u32, hwnd: isize) -> Result<Shot, String> {
        physical(|| {
            let main = handle(hwnd);
            // SAFETY: a plain query on a window handle.
            if unsafe { IsIconic(main) }.as_bool() {
                return Err("the window is minimized; restore it first".into());
            }
            let canvas = frame(main);
            let (cw, ch) = (canvas.right - canvas.left, canvas.bottom - canvas.top);
            if cw <= 0 || ch <= 0 {
                return Err("the window has no size".into());
            }
            let order = crate::os_input::top_level_windows(pid);
            let at = order
                .iter()
                .position(|&h| h == main)
                .ok_or("the window is no longer one of the app's")?;
            let mut rgba = vec![0u8; (cw * ch * 4) as usize];
            let mut windows = Vec::new();
            // SAFETY: the screen DC is released below.
            let screen = unsafe { GetDC(None) };
            for &layer in order[..=at].iter().rev() {
                // SAFETY: a plain query on a window handle.
                if unsafe { IsIconic(layer) }.as_bool() || cloaked(layer) {
                    continue;
                }
                let f = frame(layer);
                let Some((r, bgra)) = print(screen, layer) else {
                    if layer == main {
                        // SAFETY: `screen` came from GetDC(None).
                        unsafe { ReleaseDC(None, screen) };
                        return Err("Windows could not draw the window".into());
                    }
                    continue;
                };
                // What shows of this window on the canvas, inside what was drawn of it.
                let left = f.left.max(canvas.left).max(r.left);
                let top = f.top.max(canvas.top).max(r.top);
                let right = f.right.min(canvas.right).min(r.right);
                let bottom = f.bottom.min(canvas.bottom).min(r.bottom);
                if right <= left || bottom <= top {
                    continue;
                }
                let (w, span) = (r.right - r.left, (right - left) as usize * 4);
                for y in top..bottom {
                    let from = (((y - r.top) * w + (left - r.left)) * 4) as usize;
                    let to = (((y - canvas.top) * cw + (left - canvas.left)) * 4) as usize;
                    for (dst, src) in rgba[to..to + span]
                        .chunks_exact_mut(4)
                        .zip(bgra[from..from + span].chunks_exact(4))
                    {
                        dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
                    }
                }
                windows.push(Drawn {
                    title: text(layer, false),
                    class: text(layer, true),
                    rect: (
                        left - canvas.left,
                        top - canvas.top,
                        right - left,
                        bottom - top,
                    ),
                });
            }
            // SAFETY: `screen` came from GetDC(None).
            unsafe { ReleaseDC(None, screen) };
            Ok(Shot {
                width: cw as u32,
                height: ch as u32,
                rgba,
                windows,
            })
        })
    }

    /// Moves the window to `at` and sizes it to `size`, in its logical pixels, without activating
    /// it or changing its place in the stack.
    pub fn set(
        hwnd: isize,
        at: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
    ) -> Result<(), String> {
        let hwnd = handle(hwnd);
        physical(|| {
            // SAFETY: plain queries on a window handle.
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                return Err("the window is maximized; restore it first".into());
            }
            // SAFETY: as above.
            if unsafe { IsIconic(hwnd) }.as_bool() {
                return Err("the window is minimized; restore it first".into());
            }
            let scale = scale(hwnd)?;
            let px = |v: f64| (v * scale).round() as i32;
            let (x, y) = at.map_or((0, 0), |(x, y)| (px(x), px(y)));
            let (w, h) = size.map_or((0, 0), |(w, h)| (px(w), px(h)));
            let mut flags = SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER;
            if at.is_none() {
                flags |= SWP_NOMOVE;
            }
            if size.is_none() {
                flags |= SWP_NOSIZE;
            }
            // SAFETY: `hwnd` is a window of the app; no z-order change is asked for.
            unsafe { SetWindowPos(hwnd, None, x, y, w, h, flags) }
                .map_err(|e| format!("Windows did not move or resize the window: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PageBox {
        PageBox {
            x: 341.0,
            y: 317.0,
            width: 586.0,
            height: 410.0,
            ratio: 1.5,
        }
    }

    fn area(x: i32, y: i32, width: i32, height: i32) -> Area {
        Area {
            x,
            y,
            width,
            height,
            scale: 1.5,
        }
    }

    #[test]
    fn the_page_is_in_the_window_whose_page_area_matches_it_within_a_css_pixel() {
        let other = area(125, 125, 630, 480);
        let this = area(511, 475, 878, 614);
        assert_eq!(matching(page(), &[other, this]), Ok(1));
        assert!(matching(page(), &[other]).is_err());
        assert!(matching(page(), &[area(514, 475, 878, 614)]).is_err());
        assert!(matching(page(), &[this, this])
            .unwrap_err()
            .contains("cannot tell"));
    }

    #[test]
    fn a_zoomed_page_is_matched_by_its_origin_at_the_window_scale() {
        let zoomed = PageBox {
            ratio: 1.875,
            width: 468.0,
            height: 328.0,
            ..page()
        };
        assert_eq!(matching(zoomed, &[area(511, 475, 878, 614)]), Ok(0));
    }

    #[test]
    fn the_outer_size_for_a_viewport_grows_the_window_by_what_the_page_lacks() {
        let placed = Placed {
            x: 0.0,
            y: 0.0,
            width: 600.0,
            height: 466.67,
            scale: 1.5,
        };
        let (w, h) = outer_for_viewport((640.0, 400.0), 1.5, placed, area(0, 0, 878, 614));
        assert!((w - (600.0 + (960.0 - 878.0) / 1.5)).abs() < 1e-9, "{w}");
        assert!((h - (466.67 + (600.0 - 614.0) / 1.5)).abs() < 1e-9, "{h}");
    }
}

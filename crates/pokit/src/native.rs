//! An app's native UI on Windows: its menu bars, read and chosen through Win32, and its dialogs,
//! read and answered through UI Automation.

#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;

/// One menu entry, with its submenu.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub checked: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<MenuItem>,
    #[serde(skip)]
    pub id: u32,
}

/// A dialog an app has open: its title, its text and its buttons.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dialog {
    pub title: String,
    pub text: Vec<String>,
    pub buttons: Vec<String>,
    #[serde(skip)]
    pub hwnd: isize,
}

/// A menu label as shown: `&` marks dropped (`&&` kept as `&`), and the shortcut after a tab
/// split off.
pub fn clean_label(raw: &str) -> (String, Option<String>) {
    let (text, shortcut) = match raw.split_once('\t') {
        Some((t, s)) => (t, Some(s.to_string())),
        None => (raw, None),
    };
    let mut label = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                label.push('&');
                chars.next();
            }
        } else {
            label.push(c);
        }
    }
    (label, shortcut)
}

/// A menu path written `File > Say hello`.
pub fn parse_path(path: &str) -> Vec<String> {
    path.split('>')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The entry at `path` in `menu`, matched label by label.
pub fn find<'a>(menu: &'a [MenuItem], path: &[String]) -> Option<&'a MenuItem> {
    let (first, rest) = path.split_first()?;
    let item = menu.iter().find(|i| &i.label == first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        find(&item.items, rest)
    }
}

/// Whether a button's name answers to `wanted`: the same, or the same up to a `(&Y)`-style
/// access key a localised dialog adds.
pub fn button_matches(name: &str, wanted: &str) -> bool {
    let name = name.trim();
    name == wanted
        || name
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.trim_start().starts_with('('))
}

/// The menu as text, one entry per line, indented by depth.
pub fn render(menu: &[MenuItem], depth: usize, out: &mut String) {
    for item in menu {
        out.push_str(&"  ".repeat(depth));
        out.push_str("- ");
        out.push_str(&item.label);
        if let Some(s) = &item.shortcut {
            out.push_str(&format!(" ({s})"));
        }
        if !item.enabled {
            out.push_str(" [disabled]");
        }
        if item.checked {
            out.push_str(" [checked]");
        }
        out.push('\n');
        render(&item.items, depth + 1, out);
    }
}

/// A dialog button pressed: the dialog, the button's full name, and whether the dialog closed.
#[derive(Debug)]
pub struct Answered {
    pub dialog: String,
    pub pressed: String,
    pub closed: bool,
}

/// Why a menu entry was not chosen or a dialog not answered: what was named is not there, or it
/// is there and cannot be operated.
#[derive(Debug)]
pub enum Refusal {
    Missing(String),
    Refused(String),
}

#[cfg(windows)]
pub use win::{answer, choose, dialogs, menus};

#[cfg(windows)]
mod win {
    use super::{Answered, Dialog, MenuItem, Refusal};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
        TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_InvokePatternId, UIA_TextControlTypeId,
        UIA_TitleBarControlTypeId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetDlgCtrlID, GetMenu, GetMenuItemCount, GetMenuItemInfoW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW,
        SendMessageTimeoutW, HMENU, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFT_SEPARATOR,
        MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SMTO_ABORTIFHUNG, WM_COMMAND,
        WM_NULL,
    };

    /// The visible top-level windows of process `pid`.
    fn windows_of(pid: u32) -> Vec<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            // SAFETY: `lparam` is the `(pid, Vec)` passed below, alive for the enumeration.
            let found = unsafe { &mut *(lparam.0 as *mut (u32, Vec<HWND>)) };
            let mut owner = 0u32;
            // SAFETY: `hwnd` comes from EnumWindows; `owner` is a valid out-pointer.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            // SAFETY: as above.
            if owner == found.0 && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                found.1.push(hwnd);
            }
            true.into()
        }
        let mut found = (pid, Vec::new());
        // SAFETY: the callback reads `found` only while EnumWindows runs.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize)) };
        found.1
    }

    fn window_text(hwnd: HWND) -> String {
        let mut buf = [0u16; 512];
        // SAFETY: `buf` is valid for its length.
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        // SAFETY: `buf` is valid for its length.
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// Every entry of `menu`, with its submenus.
    fn read_menu(menu: HMENU) -> Vec<MenuItem> {
        // SAFETY: an invalid menu reads as having no entries.
        let count = unsafe { GetMenuItemCount(Some(menu)) };
        let mut items = Vec::new();
        for i in 0..count.max(0) as u32 {
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING,
                ..Default::default()
            };
            // SAFETY: `info` is sized and asks only for the length of the text (`cch` 0).
            if unsafe { GetMenuItemInfoW(menu, i, true, &mut info) }.is_err() {
                continue;
            }
            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                continue;
            }
            let mut text = vec![0u16; info.cch as usize + 1];
            info.cch += 1;
            info.dwTypeData = PWSTR(text.as_mut_ptr());
            // SAFETY: `text` holds `cch` units, and outlives the call.
            if unsafe { GetMenuItemInfoW(menu, i, true, &mut info) }.is_err() {
                continue;
            }
            let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
            let (label, shortcut) = super::clean_label(&raw);
            items.push(MenuItem {
                label,
                shortcut,
                enabled: info.fState.0 & MFS_DISABLED.0 == 0,
                checked: info.fState.0 & MFS_CHECKED.0 != 0,
                items: if info.hSubMenu.is_invalid() {
                    Vec::new()
                } else {
                    read_menu(info.hSubMenu)
                },
                id: info.wID,
            });
        }
        items
    }

    /// The menu bars of process `pid`'s windows: (window title, menu, window).
    pub fn menus(pid: u32) -> Vec<(String, Vec<MenuItem>, isize)> {
        windows_of(pid)
            .into_iter()
            .filter_map(|hwnd| {
                // SAFETY: `hwnd` is a live window; a window without a menu gives a null menu.
                let menu = unsafe { GetMenu(hwnd) };
                (!menu.is_invalid()).then(|| (window_text(hwnd), read_menu(menu), hwnd.0 as isize))
            })
            .collect()
    }

    /// Chooses the entry at `path` in the first menu bar of process `pid` that has it, the way
    /// clicking it would: a `WM_COMMAND` with its id, posted to its window.
    pub fn choose(pid: u32, path: &[String]) -> Result<String, Refusal> {
        for (title, menu, hwnd) in menus(pid) {
            let Some(item) = super::find(&menu, path) else {
                continue;
            };
            if !item.items.is_empty() {
                return Err(Refusal::Refused(format!(
                    "`{}` opens a submenu; choose an entry in it",
                    path.join(" > ")
                )));
            }
            if !item.enabled {
                return Err(Refusal::Refused(format!(
                    "`{}` is disabled",
                    path.join(" > ")
                )));
            }
            // SAFETY: a plain query on a window of the app.
            if !unsafe { IsWindowEnabled(HWND(hwnd as *mut core::ffi::c_void)) }.as_bool() {
                return Err(Refusal::Refused(format!(
                    "\"{title}\" is disabled, most likely behind a modal dialog; answer that first"
                )));
            }
            // SAFETY: posts to a live window of the app; nothing is borrowed.
            unsafe {
                PostMessageW(
                    Some(HWND(hwnd as *mut core::ffi::c_void)),
                    WM_COMMAND,
                    WPARAM(item.id as usize),
                    LPARAM(0),
                )
            }
            .map_err(|e| Refusal::Refused(format!("could not post the menu command: {e}")))?;
            return Ok(title);
        }
        Err(Refusal::Missing(format!(
            "no menu has `{}`",
            path.join(" > ")
        )))
    }

    /// The notification a button sends its dialog when clicked (`BN_CLICKED`, winuser.h).
    const BN_CLICKED: usize = 0;

    /// Whether `el` sits in a window's title bar, as its Close button does.
    fn in_title_bar(uia: &IUIAutomation, el: &IUIAutomationElement) -> bool {
        // SAFETY: COM calls on live elements; a failure reads as not in a title bar.
        unsafe {
            uia.RawViewWalker()
                .and_then(|w| w.GetParentElement(el))
                .and_then(|p| p.CurrentControlType())
                .is_ok_and(|t| t == UIA_TitleBarControlTypeId)
        }
    }

    fn automation() -> Result<IUIAutomation, String> {
        // SAFETY: initialising COM on this thread; an already-initialised thread is fine.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // SAFETY: creates the system's UI Automation object.
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("UI Automation is not available: {e}"))
    }

    /// The dialogs process `pid` has open, with their text and buttons.
    pub fn dialogs(pid: u32) -> Result<Vec<Dialog>, String> {
        let uia = automation()?;
        let mut out = Vec::new();
        for hwnd in windows_of(pid) {
            if class_name(hwnd) != "#32770" {
                continue;
            }
            let (mut text, mut buttons) = (Vec::new(), Vec::new());
            // SAFETY: `hwnd` was a live window when listed; every COM call is checked, and a
            // dialog that closes meanwhile is left out.
            let read = unsafe {
                (|| -> windows::core::Result<()> {
                    let root = uia.ElementFromHandle(hwnd)?;
                    let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition()?)?;
                    for i in 0..all.Length().unwrap_or(0) {
                        let Ok(el) = all.GetElement(i) else { continue };
                        if in_title_bar(&uia, &el) {
                            continue;
                        }
                        let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                        match el.CurrentControlType() {
                            Ok(t) if t == UIA_ButtonControlTypeId && !name.is_empty() => {
                                buttons.push(name)
                            }
                            Ok(t) if t == UIA_TextControlTypeId && !name.is_empty() => {
                                text.push(name)
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })()
            };
            if read.is_err() {
                continue;
            }
            out.push(Dialog {
                title: window_text(hwnd),
                text,
                buttons,
                hwnd: hwnd.0 as isize,
            });
        }
        Ok(out)
    }

    /// Presses the button named `button` in a dialog of process `pid` (the one titled `title`,
    /// when given), once the dialog's thread is taking messages: a Win32 button by posting the
    /// dialog the `WM_COMMAND` its click sends, which needs no activation, and anything else
    /// through UI Automation's Invoke;
    /// returns the dialog's title, the button's full name, and whether the dialog closed within
    /// a second.
    pub fn answer(pid: u32, button: &str, title: Option<&str>) -> Result<Answered, Refusal> {
        let uia = automation().map_err(Refusal::Refused)?;
        let open = dialogs(pid).map_err(Refusal::Refused)?;
        let dialog = open
            .iter()
            .find(|d| title.is_none_or(|t| d.title == t))
            .ok_or_else(|| {
                Refusal::Missing(match title {
                    Some(t) => format!("no open dialog is titled `{t}`"),
                    None => "the app has no dialog open".to_string(),
                })
            })?;
        let hwnd = HWND(dialog.hwnd as *mut core::ffi::c_void);
        // SAFETY: a null message to a window, with a timeout; it returns once the window's thread
        // has taken it, or after a second.
        unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                1000,
                None,
            )
        };
        // SAFETY: the dialog's window was live when listed; every COM call is checked.
        unsafe {
            let root = uia
                .ElementFromHandle(hwnd)
                .map_err(|e| Refusal::Refused(e.to_string()))?;
            let all = root
                .FindAll(
                    TreeScope_Descendants,
                    &uia.CreateTrueCondition()
                        .map_err(|e| Refusal::Refused(e.to_string()))?,
                )
                .map_err(|e| Refusal::Refused(e.to_string()))?;
            for i in 0..all.Length().unwrap_or(0) {
                let Ok(el) = all.GetElement(i) else { continue };
                if el.CurrentControlType() != Ok(UIA_ButtonControlTypeId) || in_title_bar(&uia, &el)
                {
                    continue;
                }
                let name = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
                if !super::button_matches(&name, button) {
                    continue;
                }
                let native = el.CurrentNativeWindowHandle().unwrap_or_default();
                if !native.is_invalid() && class_name(native) == "Button" {
                    let id = GetDlgCtrlID(native) as u16 as usize;
                    PostMessageW(
                        Some(hwnd),
                        WM_COMMAND,
                        WPARAM(id | (BN_CLICKED << 16)),
                        LPARAM(native.0 as isize),
                    )
                    .map_err(|e| Refusal::Refused(format!("pressing `{name}` failed: {e}")))?;
                } else {
                    let invoke: IUIAutomationInvokePattern =
                        el.GetCurrentPatternAs(UIA_InvokePatternId).map_err(|e| {
                            Refusal::Refused(format!("`{name}` cannot be pressed: {e}"))
                        })?;
                    invoke
                        .Invoke()
                        .map_err(|e| Refusal::Refused(format!("pressing `{name}` failed: {e}")))?;
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while IsWindow(Some(hwnd)).as_bool() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                return Ok(Answered {
                    dialog: dialog.title.clone(),
                    pressed: name,
                    closed: !IsWindow(Some(hwnd)).as_bool(),
                });
            }
        }
        Err(Refusal::Missing(format!(
            "`{}` has no button `{button}`; its buttons are {:?}",
            dialog.title, dialog.buttons
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, items: Vec<MenuItem>) -> MenuItem {
        MenuItem {
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: false,
            items,
            id: 0,
        }
    }

    #[test]
    fn labels_lose_their_access_keys_and_keep_their_shortcut_apart() {
        assert_eq!(
            clean_label("Say &hello\tCtrl+H"),
            ("Say hello".into(), Some("Ctrl+H".into()))
        );
        assert_eq!(clean_label("&File"), ("File".into(), None));
        assert_eq!(clean_label("Save && Quit"), ("Save & Quit".into(), None));
    }

    #[test]
    fn a_path_is_found_label_by_label() {
        let menu = vec![item(
            "File",
            vec![item("Say hello", vec![]), item("Not now", vec![])],
        )];
        assert_eq!(parse_path(" File >  Say hello "), vec!["File", "Say hello"]);
        assert_eq!(
            find(&menu, &parse_path("File > Say hello")).unwrap().label,
            "Say hello"
        );
        assert!(find(&menu, &parse_path("File > Missing")).is_none());
        assert!(find(&menu, &parse_path("Help")).is_none());
    }

    #[test]
    fn a_localised_button_answers_to_its_name_without_the_access_key() {
        assert!(button_matches("Yes", "Yes"));
        assert!(button_matches("예(Y)", "예"));
        assert!(button_matches("아니요(N)", "아니요"));
        assert!(!button_matches("Yes", "Ye"));
        assert!(!button_matches("No", "Yes"));
    }

    #[test]
    fn a_menu_renders_one_entry_per_line_with_its_state() {
        let mut off = item("Not now", vec![]);
        off.enabled = false;
        let mut hello = item("Say hello", vec![]);
        hello.shortcut = Some("Ctrl+H".into());
        let mut out = String::new();
        render(&[item("File", vec![hello, off])], 0, &mut out);
        assert_eq!(
            out,
            "- File\n  - Say hello (Ctrl+H)\n  - Not now [disabled]\n"
        );
    }
}

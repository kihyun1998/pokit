//! The fixture app: a webview app whose every behaviour has a known answer,
//! so pokit's tests can assert what pokit reports against it.

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Prints a known line on the backend's stdout.
#[tauri::command]
fn backend_log(line: String) {
    println!("fixture-backend: {line}");
}

/// Opens the second window, or does nothing if it is already open.
#[tauri::command]
async fn open_second(app: tauri::AppHandle) -> Result<(), String> {
    if app.get_webview_window("second").is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "second", WebviewUrl::App("second.html".into()))
        .title("pokit fixture - second")
        .inner_size(420.0, 320.0)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens a window where something was dropped outside the main one, at (`x`, `y`) in logical
/// screen pixels, without taking the focus.
#[tauri::command]
async fn open_dropped(app: tauri::AppHandle, x: f64, y: f64) -> Result<(), String> {
    if app.get_webview_window("dropped").is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "dropped", WebviewUrl::App("second.html".into()))
        .title("pokit fixture - dropped")
        .position(x, y)
        .inner_size(320.0, 200.0)
        .focused(false)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Pops up the page's context menu at the cursor: Mark here, a disabled entry, and a submenu
/// with Deeper.
#[tauri::command]
fn show_context_menu(window: tauri::Window) -> Result<(), String> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    let build = || -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
        let mark = MenuItemBuilder::with_id("ctx-mark", "&Mark here").build(&window)?;
        let off = MenuItemBuilder::with_id("ctx-off", "Cannot")
            .enabled(false)
            .build(&window)?;
        let deeper = MenuItemBuilder::with_id("ctx-deeper", "Deeper").build(&window)?;
        let more = SubmenuBuilder::new(&window, "More").item(&deeper).build()?;
        MenuBuilder::new(&window)
            .items(&[&mark, &off, &more])
            .build()
    };
    let menu = build().map_err(|e| e.to_string())?;
    window.popup_menu(&menu).map_err(|e| e.to_string())
}

/// Hides the main window's webview for `ms` milliseconds, as a host does when its window is
/// minimized (`IsVisible` false), without touching the window itself.
#[tauri::command]
async fn hide_webview_for(app: tauri::AppHandle, ms: u64) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("the main window is gone")?;
    set_webview_visible(&window, false);
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        set_webview_visible(&window, true);
    });
    Ok(())
}

#[cfg(windows)]
fn set_webview_visible(window: &tauri::WebviewWindow, visible: bool) {
    let _ = window.with_webview(move |webview| {
        // SAFETY: the controller is live for the window's lifetime.
        let _ = unsafe { webview.controller().SetIsVisible(visible) };
    });
}

#[cfg(not(windows))]
fn set_webview_visible(_window: &tauri::WebviewWindow, _visible: bool) {}

/// Writes `text` into the page's native-result line.
fn show_native_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval(format!(
            "document.querySelector('#native-result').textContent = {text:?}"
        ));
    }
}

/// The main window's menu: File > Say hello, a disabled item, and Help > Ask, which asks a
/// yes-or-no question in a native dialog.
fn native_menu(app: &tauri::App) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    let hello = MenuItemBuilder::with_id("hello", "Say &hello")
        .accelerator("Ctrl+H")
        .build(app)?;
    let off = MenuItemBuilder::with_id("off", "Not now")
        .enabled(false)
        .build(app)?;
    let ask = MenuItemBuilder::with_id("ask", "&Ask").build(app)?;
    let file = SubmenuBuilder::new(app, "&File")
        .item(&hello)
        .separator()
        .item(&off)
        .build()?;
    let help = SubmenuBuilder::new(app, "&Help").item(&ask).build()?;
    MenuBuilder::new(app).items(&[&file, &help]).build()
}

/// Asks "Proceed?" with Yes and No in a native dialog owned by the main window, and shows the
/// answer on the page.
#[cfg(windows)]
fn ask_in_a_dialog(app: tauri::AppHandle) {
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_YESNO};
    let owner = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0);
    std::thread::spawn(move || {
        // SAFETY: both strings are static and NUL-terminated; the owner may be gone, which only
        // makes the dialog ownerless.
        let answer = unsafe {
            MessageBoxW(
                Some(HWND(owner as *mut core::ffi::c_void)),
                w!("Proceed?"),
                w!("pokit fixture question"),
                MB_YESNO,
            )
        };
        show_native_result(
            &app,
            if answer == IDYES {
                "answered yes"
            } else {
                "answered no"
            },
        );
    });
}

#[cfg(not(windows))]
fn ask_in_a_dialog(_app: tauri::AppHandle) {}

/// Prints every key WebView2 raises through `AcceleratorKeyPressed` on the main window.
#[cfg(windows)]
fn log_accelerator_keys(window: &tauri::WebviewWindow) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_KEY_EVENT_KIND;
    let _ = window.with_webview(|webview| {
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                let (mut vk, mut kind) = (0u32, COREWEBVIEW2_KEY_EVENT_KIND::default());
                // SAFETY: both out-pointers are valid locals for the duration of the call.
                unsafe {
                    args.VirtualKey(&mut vk)?;
                    args.KeyEventKind(&mut kind)?;
                }
                println!("fixture-accelerator: vk={vk} kind={}", kind.0);
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: the controller is live for the window's lifetime and `token` is a valid out-pointer.
        let _ = unsafe {
            webview
                .controller()
                .add_AcceleratorKeyPressed(&handler, &mut token)
        };
    });
}

fn main() {
    println!("fixture-backend: started");
    let builder = tauri::Builder::default();
    #[cfg(feature = "pokit")]
    let builder = builder.plugin(tauri_plugin_pokit::init());
    #[cfg(all(feature = "pokit", target_os = "macos"))]
    let builder = builder.activate_ignoring_other_apps(false);
    builder
        .setup(|app| {
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                log_accelerator_keys(&window);
            }
            let menu = native_menu(app)?;
            if let Some(window) = app.get_webview_window("main") {
                window.set_menu(menu)?;
            }
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "hello" => show_native_result(app, "hello from the menu"),
                "ask" => ask_in_a_dialog(app.clone()),
                "ctx-mark" => show_native_result(app, "marked from the context menu"),
                "ctx-deeper" => show_native_result(app, "deeper from the context menu"),
                _ => {}
            });
            if std::env::args().any(|a| a == "--take-focus") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_focus();
                }
            }
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_log,
            open_second,
            open_dropped,
            show_context_menu,
            hide_webview_for
        ])
        .run(tauri::generate_context!())
        .expect("fixture app failed to run");
}

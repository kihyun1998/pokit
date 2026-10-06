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
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                log_accelerator_keys(&window);
            }
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
            hide_webview_for
        ])
        .run(tauri::generate_context!())
        .expect("fixture app failed to run");
}

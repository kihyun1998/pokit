//! pokit's plugin for a Tauri app's test build.
//!
//! An app adds it behind a feature of its own, so only a test build carries it:
//!
//! ```ignore
//! #[cfg(feature = "pokit")]
//! let builder = builder.plugin(tauri_plugin_pokit::init());
//! ```
//!
//! It opens its channel only when `pokit launch` started the app: [`TOKEN_VAR`] holds the token
//! every request must carry, and [`PORT_FILE_VAR`] the file it writes its loopback port to.

mod channel;
#[cfg(target_os = "macos")]
mod eval;

use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The environment variable holding the token every request must carry.
pub const TOKEN_VAR: &str = "POKIT_TOKEN";
/// The environment variable naming the file the plugin writes its port to.
pub const PORT_FILE_VAR: &str = "POKIT_PORT_FILE";

/// The plugin. It does nothing unless both [`TOKEN_VAR`] and [`PORT_FILE_VAR`] are set.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("pokit")
        .setup(|app, _api| {
            channel::open(app);
            Ok(())
        })
        .build()
}

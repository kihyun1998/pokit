//! Which commands each platform answers, and why a command is refused where it is not.

use crate::output::{Failure, Kind};

/// The commands the macOS backend answers, by their command-line name.
const ON_MACOS: &[&str] = &[
    "capabilities",
    "launch",
    "close",
    "eval",
    "read",
    "wait",
    "click",
    "key",
    "type",
];

/// Why trace and profile are refused on macOS.
const NO_TIMELINE: &str =
    "unsupported on macOS: there is no scripted path to WebKit's Timeline or profiler (#1)";

/// The command-line name a request kind belongs to: `trace_start` → `trace`.
pub fn command_of(kind_name: &str) -> &str {
    kind_name.split('_').next().unwrap_or(kind_name)
}

/// Whether this platform answers `command` (a command-line name).
pub fn answers(command: &str) -> bool {
    if cfg!(windows) {
        return true;
    }
    cfg!(target_os = "macos") && ON_MACOS.contains(&command)
}

/// Whether a session that reaches the page through the plugin answers a request of kind
/// `kind_name`.
pub fn plugin_answers(kind_name: &str) -> bool {
    matches!(kind_name, "ping" | "status") || ON_MACOS.contains(&command_of(kind_name))
}

/// Whether a session on this platform answers a request of kind `kind_name`.
pub fn session_answers(kind_name: &str) -> bool {
    matches!(kind_name, "ping" | "status") || answers(command_of(kind_name))
}

/// How input reaches the page for `command` (a command-line name), route by route, and whether
/// each route takes the user's focus (#1, story 42); `None` for a command that is not input.
pub fn routes(command: &str) -> Option<serde_json::Value> {
    use serde_json::json;
    let cdp = json!({ "default": true, "takes_focus": false });
    let os = json!({ "takes_focus": true, "needs": "the app in front: `window activate`" });
    match command {
        "click" | "key" if cfg!(windows) => Some(json!({ "cdp": cdp, "os": os })),
        "type" if cfg!(windows) => Some(json!({
            "cdp": { "default": true, "takes_focus": false, "ime_composition": true },
            "os": { "takes_focus": true, "needs": "the app in front: `window activate`", "ime_composition": false },
        })),
        "click" if cfg!(target_os = "macos") => Some(json!({
            "plugin": {
                "default": true,
                "takes_focus": false,
                "right_click_menu": "the menu WebKit opens, when the page does not prevent it, is closed at once (`menu_closed`); a menu the app opens stays",
            },
        })),
        "key" if cfg!(target_os = "macos") => Some(json!({
            "plugin": { "default": true, "takes_focus": false },
        })),
        "type" if cfg!(target_os = "macos") => Some(json!({
            "plugin": { "default": true, "takes_focus": false, "ime_composition": false },
        })),
        "hold" | "wheel" if cfg!(windows) => Some(json!({ "cdp": cdp, "os": os })),
        "drag" if cfg!(windows) => Some(json!({
            "cdp": { "default": true, "takes_focus": false, "leaves_the_page": false, "html5_drag": "always" },
            "os": {
                "takes_focus": true,
                "needs": "the app in front: `window activate`",
                "leaves_the_page": true,
                "html5_drag": "only where the window's drop target takes drags without files: not under Tauri's default `dragDropEnabled`",
            },
        })),
        _ => None,
    }
}

/// Why `command` (a command-line name) is refused on this platform.
pub fn reason(command: &str) -> &'static str {
    if cfg!(target_os = "macos") {
        match command {
            "trace" | "profile" => NO_TIMELINE,
            _ => "not built on macOS yet (#6)",
        }
    } else {
        "only the Windows and macOS backends exist"
    }
}

/// Why a session refuses `command`'s input by `route`: the plugin's session takes only the plugin
/// route, a CDP session every route but the plugin's. `None` when it takes it.
pub fn route_refused(
    route: crate::request::Route,
    through_plugin: bool,
    command: &str,
) -> Option<Failure> {
    use crate::request::Route;
    let why = match (route, through_plugin) {
        (Route::Plugin, true) | (Route::Cdp | Route::Os, false) => return None,
        (Route::Cdp, true) => "there is no CDP on macOS: input goes through the app's plugin, the default route (`--route plugin`)",
        (Route::Os, true) if command == "click" => "there is no OS click on macOS that leaves the cursor alone: clicks go through the app's plugin (`--route plugin`)",
        (Route::Os, true) => "OS input is not built on macOS yet",
        (Route::Plugin, false) => return Some(plugin_route_on_cdp()),
    };
    Some(Failure::new(Kind::Unsupported, why).with("platform", std::env::consts::OS))
}

/// The plugin route asked of a session that reaches the page over CDP.
pub fn plugin_route_on_cdp() -> Failure {
    Failure::new(
        Kind::Unsupported,
        "the plugin route is macOS's; this session reaches the page over CDP",
    )
    .with("platform", std::env::consts::OS)
}

/// The failure for `command` (a command-line name, or a request kind) refused here.
pub fn refused(command: &str) -> Failure {
    Failure::new(Kind::Unsupported, reason(command_of(command)))
        .with("platform", std::env::consts::OS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_kind_belongs_to_its_command() {
        assert_eq!(command_of("trace_start"), "trace");
        assert_eq!(command_of("measure_stop"), "measure");
        assert_eq!(command_of("eval"), "eval");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_clicks_through_the_plugin_and_names_no_os_route() {
        assert!(answers("click") && plugin_answers("click"));
        let routes = routes("click").unwrap();
        assert_eq!(routes["plugin"]["default"], true);
        assert_eq!(routes["plugin"]["takes_focus"], false);
        assert!(routes.get("os").is_none(), "{routes}");
        let os = route_refused(crate::request::Route::Os, true, "click").unwrap();
        assert!(os.message.contains("cursor"), "{}", os.message);
        let key = route_refused(crate::request::Route::Os, true, "key").unwrap();
        assert!(key.message.contains("not built"), "{}", key.message);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn trace_and_profile_carry_the_timeline_reason_by_either_name() {
        for name in ["trace", "trace_start", "profile_stop"] {
            assert_eq!(refused(name).message, NO_TIMELINE, "{name}");
        }
        assert!(!session_answers("trace_start"));
        assert!(session_answers("eval") && session_answers("status"));
    }
}

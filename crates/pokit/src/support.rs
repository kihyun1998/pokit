//! Which commands each platform answers, and why a command is refused where it is not.

use crate::output::{Failure, Kind};

/// The commands the macOS backend answers, by their command-line name.
const ON_MACOS: &[&str] = &["capabilities", "launch", "close", "eval"];

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
        "hold" | "wheel" if cfg!(windows) => Some(json!({ "cdp": cdp, "os": os })),
        "drag" if cfg!(windows) => Some(json!({
            "cdp": { "default": true, "takes_focus": false, "leaves_the_page": false },
            "os": { "takes_focus": true, "needs": "the app in front: `window activate`", "leaves_the_page": true },
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
    fn trace_and_profile_carry_the_timeline_reason_by_either_name() {
        for name in ["trace", "trace_start", "profile_stop"] {
            assert_eq!(refused(name).message, NO_TIMELINE, "{name}");
        }
        assert!(!session_answers("trace_start"));
        assert!(session_answers("eval") && session_answers("status"));
    }
}

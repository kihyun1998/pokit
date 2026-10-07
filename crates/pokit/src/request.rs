//! The commands a session answers, as the CLI sends them and the session receives them.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long `hold` waits for the page to acknowledge its keys after sending the last one.
pub const HOLD_ACK_WAIT: Duration = Duration::from_secs(15);

/// How much longer than its own timeout the CLI waits for `trace stop`, which reads the trace.
pub const TRACE_STOP_WAIT: Duration = Duration::from_secs(60);

/// How input reaches the page: CDP, straight into the page, or OS input through `SendInput`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    #[default]
    Cdp,
    Os,
}

/// How long each measured run of `hold --compare` may wait for the page to settle.
pub const COMPARE_SETTLE: Duration = Duration::from_secs(10);

/// One command for the session, with its arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Close,
    Targets {
        select: Option<String>,
    },
    Snapshot,
    Read {
        target: String,
    },
    Click {
        target: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        double: bool,
        right: bool,
        hover: bool,
        require_focus: Option<String>,
        #[serde(default)]
        route: Route,
    },
    Type {
        text: String,
        secret: bool,
        into: Option<String>,
        require_focus: Option<String>,
        #[serde(default)]
        route: Route,
    },
    Key {
        chord: String,
        into: Option<String>,
        require_focus: Option<String>,
        #[serde(default)]
        route: Route,
    },
    Hold {
        chord: String,
        count: u32,
        interval_ms: u64,
        into: Option<String>,
        require_focus: Option<String>,
        #[serde(default)]
        route: Route,
        #[serde(default)]
        compare: bool,
    },
    Wheel {
        target: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        notches: i32,
        #[serde(default)]
        route: Route,
    },
    MeasureStart {
        watch: Option<String>,
        watch_attr: Option<String>,
        long_frame_ms: f64,
        over_ms: f64,
    },
    MeasureStop {
        quiet_ms: u64,
        ceiling_ms: u64,
    },
    ClipboardRead,
    ClipboardWrite {
        text: String,
        secret: bool,
    },
    WindowActivate,
    WindowMove {
        x: f64,
        y: f64,
    },
    WindowResize {
        width: f64,
        height: f64,
        /// The size is the page's viewport in CSS pixels, not the window's.
        #[serde(default)]
        viewport: bool,
    },
    NativeList,
    NativeChoose {
        path: String,
    },
    NativeAnswer {
        button: String,
        dialog: Option<String>,
    },
    TraceStart,
    TraceStop,
    ProfileStart {
        interval_us: u64,
    },
    ProfileStop {
        top: usize,
    },
    Wait {
        selector: Option<String>,
        text: Option<String>,
        expr: Option<String>,
        timeout_ms: u64,
    },
    Capture {
        target: Option<String>,
        out: Option<String>,
    },
    Logs {
        since: Option<u64>,
    },
    Eval {
        expression: String,
        timeout_ms: u64,
    },
    Doctor,
}

/// Which command a request is, without its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Ping,
    Status,
    Close,
    Targets,
    Snapshot,
    Read,
    Click,
    Type,
    Key,
    Hold,
    Wheel,
    MeasureStart,
    MeasureStop,
    ClipboardRead,
    ClipboardWrite,
    WindowActivate,
    WindowMove,
    WindowResize,
    NativeList,
    NativeChoose,
    NativeAnswer,
    TraceStart,
    TraceStop,
    ProfileStart,
    ProfileStop,
    Wait,
    Capture,
    Logs,
    Eval,
    Doctor,
}

impl Request {
    /// Whether the command sends input through the OS, on the user's keyboard and mouse.
    pub fn sends_os_input(&self) -> bool {
        match self {
            Request::Click { route, .. }
            | Request::Type { route, .. }
            | Request::Key { route, .. }
            | Request::Wheel { route, .. } => *route == Route::Os,
            Request::Hold { route, compare, .. } => *route == Route::Os || *compare,
            _ => false,
        }
    }

    pub fn kind(&self) -> CommandKind {
        match self {
            Request::Ping => CommandKind::Ping,
            Request::Status => CommandKind::Status,
            Request::Close => CommandKind::Close,
            Request::Targets { .. } => CommandKind::Targets,
            Request::Snapshot => CommandKind::Snapshot,
            Request::Read { .. } => CommandKind::Read,
            Request::Click { .. } => CommandKind::Click,
            Request::Type { .. } => CommandKind::Type,
            Request::Key { .. } => CommandKind::Key,
            Request::Hold { .. } => CommandKind::Hold,
            Request::Wheel { .. } => CommandKind::Wheel,
            Request::MeasureStart { .. } => CommandKind::MeasureStart,
            Request::MeasureStop { .. } => CommandKind::MeasureStop,
            Request::ClipboardRead => CommandKind::ClipboardRead,
            Request::ClipboardWrite { .. } => CommandKind::ClipboardWrite,
            Request::WindowActivate => CommandKind::WindowActivate,
            Request::WindowMove { .. } => CommandKind::WindowMove,
            Request::WindowResize { .. } => CommandKind::WindowResize,
            Request::NativeList => CommandKind::NativeList,
            Request::NativeChoose { .. } => CommandKind::NativeChoose,
            Request::NativeAnswer { .. } => CommandKind::NativeAnswer,
            Request::TraceStart => CommandKind::TraceStart,
            Request::TraceStop => CommandKind::TraceStop,
            Request::ProfileStart { .. } => CommandKind::ProfileStart,
            Request::ProfileStop { .. } => CommandKind::ProfileStop,
            Request::Wait { .. } => CommandKind::Wait,
            Request::Capture { .. } => CommandKind::Capture,
            Request::Logs { .. } => CommandKind::Logs,
            Request::Eval { .. } => CommandKind::Eval,
            Request::Doctor => CommandKind::Doctor,
        }
    }

    /// How much longer than its own timeout the CLI waits for an answer.
    pub fn extra_wait(&self) -> Duration {
        match self {
            Request::Wait { timeout_ms, .. } | Request::Eval { timeout_ms, .. } => {
                Duration::from_millis(*timeout_ms)
            }
            Request::Hold {
                count,
                interval_ms,
                compare,
                ..
            } => {
                let one = Duration::from_millis(u64::from(*count) * interval_ms) + HOLD_ACK_WAIT;
                if *compare {
                    (one + COMPARE_SETTLE) * 2
                } else {
                    one
                }
            }
            Request::MeasureStop {
                quiet_ms,
                ceiling_ms,
            } => Duration::from_millis(quiet_ms + ceiling_ms),
            Request::TraceStop => TRACE_STOP_WAIT,
            _ => Duration::ZERO,
        }
    }

    /// The text a `type --secret` carries, which is masked everywhere.
    pub fn secret(&self) -> Option<&str> {
        match self {
            Request::Type {
                text, secret: true, ..
            }
            | Request::ClipboardWrite { text, secret: true } => Some(text),
            _ => None,
        }
    }
}

impl CommandKind {
    /// The command's name, as printed in every answer.
    pub fn name(self) -> &'static str {
        match self {
            CommandKind::Ping => "ping",
            CommandKind::Status => "status",
            CommandKind::Close => "close",
            CommandKind::Targets => "targets",
            CommandKind::Snapshot => "snapshot",
            CommandKind::Read => "read",
            CommandKind::Click => "click",
            CommandKind::Type => "type",
            CommandKind::Key => "key",
            CommandKind::Hold => "hold",
            CommandKind::Wheel => "wheel",
            CommandKind::MeasureStart => "measure_start",
            CommandKind::MeasureStop => "measure_stop",
            CommandKind::ClipboardRead => "clipboard_read",
            CommandKind::ClipboardWrite => "clipboard_write",
            CommandKind::WindowActivate => "window_activate",
            CommandKind::WindowMove => "window_move",
            CommandKind::WindowResize => "window_resize",
            CommandKind::NativeList => "native_list",
            CommandKind::NativeChoose => "native_choose",
            CommandKind::NativeAnswer => "native_answer",
            CommandKind::TraceStart => "trace_start",
            CommandKind::TraceStop => "trace_stop",
            CommandKind::ProfileStart => "profile_start",
            CommandKind::ProfileStop => "profile_stop",
            CommandKind::Wait => "wait",
            CommandKind::Capture => "capture",
            CommandKind::Logs => "logs",
            CommandKind::Eval => "eval",
            CommandKind::Doctor => "doctor",
        }
    }

    /// Whether a failure leaves a screenshot in the run record.
    pub fn screenshots_failure(self) -> bool {
        match self {
            CommandKind::Read
            | CommandKind::Click
            | CommandKind::Type
            | CommandKind::Key
            | CommandKind::Hold
            | CommandKind::Wheel
            | CommandKind::Wait
            | CommandKind::Eval
            | CommandKind::Snapshot => true,
            CommandKind::Ping
            | CommandKind::Status
            | CommandKind::Close
            | CommandKind::Targets
            | CommandKind::MeasureStart
            | CommandKind::MeasureStop
            | CommandKind::ClipboardRead
            | CommandKind::ClipboardWrite
            | CommandKind::WindowActivate
            | CommandKind::WindowMove
            | CommandKind::WindowResize
            | CommandKind::NativeList
            | CommandKind::NativeChoose
            | CommandKind::NativeAnswer
            | CommandKind::TraceStart
            | CommandKind::TraceStop
            | CommandKind::ProfileStart
            | CommandKind::ProfileStop
            | CommandKind::Capture
            | CommandKind::Logs
            | CommandKind::Doctor => false,
        }
    }

    /// Whether it resolves elements to remote objects, which are released when it ends.
    pub fn resolves_elements(self) -> bool {
        match self {
            CommandKind::Read
            | CommandKind::Click
            | CommandKind::Type
            | CommandKind::Key
            | CommandKind::Hold
            | CommandKind::Wheel
            | CommandKind::Capture => true,
            CommandKind::Ping
            | CommandKind::Status
            | CommandKind::Close
            | CommandKind::Targets
            | CommandKind::MeasureStart
            | CommandKind::MeasureStop
            | CommandKind::ClipboardRead
            | CommandKind::ClipboardWrite
            | CommandKind::WindowActivate
            | CommandKind::WindowMove
            | CommandKind::WindowResize
            | CommandKind::NativeList
            | CommandKind::NativeChoose
            | CommandKind::NativeAnswer
            | CommandKind::TraceStart
            | CommandKind::TraceStop
            | CommandKind::ProfileStart
            | CommandKind::ProfileStop
            | CommandKind::Snapshot
            | CommandKind::Wait
            | CommandKind::Logs
            | CommandKind::Eval
            | CommandKind::Doctor => false,
        }
    }

    /// Whether it goes into the run record, and `capabilities` lists it.
    pub fn is_public(self) -> bool {
        match self {
            CommandKind::Ping | CommandKind::Status => false,
            CommandKind::Close
            | CommandKind::Targets
            | CommandKind::Snapshot
            | CommandKind::Read
            | CommandKind::Click
            | CommandKind::Type
            | CommandKind::Key
            | CommandKind::Hold
            | CommandKind::Wheel
            | CommandKind::MeasureStart
            | CommandKind::MeasureStop
            | CommandKind::ClipboardRead
            | CommandKind::ClipboardWrite
            | CommandKind::WindowActivate
            | CommandKind::WindowMove
            | CommandKind::WindowResize
            | CommandKind::NativeList
            | CommandKind::NativeChoose
            | CommandKind::NativeAnswer
            | CommandKind::TraceStart
            | CommandKind::TraceStop
            | CommandKind::ProfileStart
            | CommandKind::ProfileStop
            | CommandKind::Wait
            | CommandKind::Capture
            | CommandKind::Logs
            | CommandKind::Eval
            | CommandKind::Doctor => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_name_of_each_request_is_its_kind_name() {
        let samples = [
            Request::Ping,
            Request::Status,
            Request::Close,
            Request::Targets { select: None },
            Request::Snapshot,
            Request::Read {
                target: "#a".into(),
            },
            Request::Click {
                target: None,
                x: Some(1.0),
                y: Some(2.0),
                double: false,
                right: false,
                hover: false,
                require_focus: None,
                route: Route::Os,
            },
            Request::Type {
                text: "a".into(),
                secret: false,
                into: None,
                require_focus: None,
                route: Route::Cdp,
            },
            Request::Key {
                chord: "Enter".into(),
                into: None,
                require_focus: None,
                route: Route::Os,
            },
            Request::Hold {
                chord: "Ctrl+Equal".into(),
                count: 3,
                interval_ms: 33,
                into: None,
                require_focus: None,
                route: Route::Os,
                compare: false,
            },
            Request::Wheel {
                target: None,
                x: Some(1.0),
                y: Some(2.0),
                notches: 3,
                route: Route::Cdp,
            },
            Request::MeasureStart {
                watch: None,
                watch_attr: None,
                long_frame_ms: 25.0,
                over_ms: 50.0,
            },
            Request::MeasureStop {
                quiet_ms: 1000,
                ceiling_ms: 10_000,
            },
            Request::ClipboardRead,
            Request::ClipboardWrite {
                text: "a".into(),
                secret: false,
            },
            Request::WindowActivate,
            Request::WindowMove { x: 1.0, y: 2.0 },
            Request::WindowResize {
                width: 3.0,
                height: 4.0,
                viewport: true,
            },
            Request::NativeList,
            Request::NativeChoose {
                path: "File > Open".into(),
            },
            Request::NativeAnswer {
                button: "Yes".into(),
                dialog: None,
            },
            Request::TraceStart,
            Request::TraceStop,
            Request::ProfileStart { interval_us: 100 },
            Request::ProfileStop { top: 20 },
            Request::Wait {
                selector: Some("#a".into()),
                text: None,
                expr: None,
                timeout_ms: 1,
            },
            Request::Capture {
                target: None,
                out: None,
            },
            Request::Logs { since: None },
            Request::Eval {
                expression: "1".into(),
                timeout_ms: 1,
            },
            Request::Doctor,
        ];
        for r in samples {
            let wire = serde_json::to_value(&r).unwrap();
            assert_eq!(wire["command"], r.kind().name(), "{wire}");
            let back: Request = serde_json::from_value(wire).unwrap();
            assert_eq!(back.kind(), r.kind());
        }
    }
}

//! The commands a session answers, as the CLI sends them and the session receives them.

use serde::{Deserialize, Serialize};
use std::time::Duration;

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
    },
    Type {
        text: String,
        secret: bool,
        into: Option<String>,
        require_focus: Option<String>,
    },
    Key {
        chord: String,
        into: Option<String>,
        require_focus: Option<String>,
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
    Wait,
    Capture,
    Logs,
    Eval,
    Doctor,
}

impl Request {
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
            _ => Duration::ZERO,
        }
    }

    /// The text a `type --secret` carries, which is masked everywhere.
    pub fn secret(&self) -> Option<&str> {
        match self {
            Request::Type {
                text, secret: true, ..
            } => Some(text),
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
            | CommandKind::Wait
            | CommandKind::Eval
            | CommandKind::Snapshot => true,
            CommandKind::Ping
            | CommandKind::Status
            | CommandKind::Close
            | CommandKind::Targets
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
            | CommandKind::Capture => true,
            CommandKind::Ping
            | CommandKind::Status
            | CommandKind::Close
            | CommandKind::Targets
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
            },
            Request::Type {
                text: "a".into(),
                secret: false,
                into: None,
                require_focus: None,
            },
            Request::Key {
                chord: "Enter".into(),
                into: None,
                require_focus: None,
            },
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

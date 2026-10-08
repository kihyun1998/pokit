//! The JSON every command prints, and the exit code that goes with it.

use serde_json::{Map, Value};

/// What went wrong, as a caller branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Error,
    Usage,
    CheckFailed,
    Timeout,
    NotFound,
    StaleRef,
    NoSession,
    GuardRefused,
    Unsupported,
    SessionExists,
    JsError,
    Reloaded,
    NotDelivered,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Error => "error",
            Kind::Usage => "usage",
            Kind::CheckFailed => "check_failed",
            Kind::Timeout => "timeout",
            Kind::NotFound => "not_found",
            Kind::StaleRef => "stale_ref",
            Kind::NoSession => "no_session",
            Kind::GuardRefused => "guard_refused",
            Kind::Unsupported => "unsupported",
            Kind::SessionExists => "session_exists",
            Kind::JsError => "js_error",
            Kind::Reloaded => "page_reloaded",
            Kind::NotDelivered => "not_delivered",
        }
    }

    /// 0 success, 1 error, 2 usage, 3 check failed, 4 timeout, 5 not found, 6 guard refused, 7 unsupported,
    /// 8 page reloaded mid-run, 9 input the page did not receive.
    pub fn exit_code(self) -> i32 {
        match self {
            Kind::Error | Kind::SessionExists | Kind::JsError => 1,
            Kind::Usage => 2,
            Kind::CheckFailed => 3,
            Kind::Timeout => 4,
            Kind::NotFound | Kind::StaleRef | Kind::NoSession => 5,
            Kind::GuardRefused => 6,
            Kind::Unsupported => 7,
            Kind::Reloaded => 8,
            Kind::NotDelivered => 9,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
    pub detail: Map<String, Value>,
}

impl Failure {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Failure {
            kind,
            message: message.into(),
            detail: Map::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.detail.insert(key.to_string(), value.into());
        self
    }
}

impl From<crate::cdp::CdpError> for Failure {
    fn from(e: crate::cdp::CdpError) -> Self {
        let kind = match e {
            crate::cdp::CdpError::Timeout(_) => Kind::Timeout,
            _ => Kind::Error,
        };
        Failure::new(kind, e.to_string())
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::new(Kind::Error, message)
    }
}

pub type Fields = Map<String, Value>;
pub type Outcome = Result<Fields, Failure>;

/// Builds an object from `key => value` pairs.
#[macro_export]
macro_rules! fields {
    ($($k:expr => $v:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut m = serde_json::Map::new();
        $( m.insert($k.to_string(), serde_json::json!($v)); )*
        m
    }};
}

/// What reaches the page: CDP on Windows, the test build's plugin on macOS.
pub const ENGINE: &str = if cfg!(target_os = "macos") {
    "plugin"
} else {
    "cdp"
};

/// The printed object and exit code for one command's outcome.
pub fn render(command: &str, outcome: &Outcome) -> (i32, Value) {
    let mut out = Map::new();
    out.insert("ok".into(), Value::Bool(outcome.is_ok()));
    out.insert("command".into(), Value::String(command.into()));
    out.insert("engine".into(), Value::String(ENGINE.into()));
    match outcome {
        Ok(fields) => {
            for (k, v) in fields {
                out.insert(k.clone(), v.clone());
            }
            (0, Value::Object(out))
        }
        Err(f) => {
            let mut err = Map::new();
            err.insert("kind".into(), Value::String(f.kind.name().into()));
            err.insert("message".into(), Value::String(f.message.clone()));
            for (k, v) in &f.detail {
                err.insert(k.clone(), v.clone());
            }
            out.insert("error".into(), Value::Object(err));
            (f.kind.exit_code(), Value::Object(out))
        }
    }
}

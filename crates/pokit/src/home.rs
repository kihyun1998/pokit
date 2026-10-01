//! pokit's home directory and the session file that lets commands find the session.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `POKIT_HOME`, else `%LOCALAPPDATA%\pokit`, else a `pokit` folder in the temp directory.
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("POKIT_HOME") {
        return PathBuf::from(h);
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(l).join("pokit");
    }
    std::env::temp_dir().join("pokit")
}

pub fn session_file() -> PathBuf {
    home().join("session.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// The session process.
    pub pid: u32,
    /// `starting`, `ready` or `failed`.
    pub status: String,
    pub error: Option<String>,
    /// Loopback port and token the session serves requests on.
    pub port: u16,
    pub token: String,
    /// `launch` or `attach`.
    pub mode: String,
    pub app_pid: Option<u32>,
    pub app_exe: Option<String>,
    /// When the launched app started, as a Windows FILETIME; identifies it beyond its pid.
    #[serde(default)]
    pub app_started: Option<u64>,
    pub cdp_port: u16,
    /// What the session answers `hello` with, so a command can tell it from another process on its port.
    #[serde(default)]
    pub proof: String,
}

pub fn read_session() -> Option<SessionInfo> {
    let text = std::fs::read_to_string(session_file()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_session(info: &SessionInfo) -> std::io::Result<()> {
    let path = session_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(info).unwrap())?;
    std::fs::rename(tmp, path)
}

/// Removes the session file if it still belongs to session process `pid`.
pub fn remove_session_if(pid: u32) {
    if read_session().map(|s| s.pid == pid).unwrap_or(false) {
        let _ = std::fs::remove_file(session_file());
    }
}

/// A UTC timestamp `YYYYMMDD-HHMMSS` for folder names.
pub fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

/// 128 random bits as hex, from the standard library's OS-seeded hasher keys.
pub fn token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let part = || {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

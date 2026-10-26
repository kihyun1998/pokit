//! Process operations the session needs: detached spawning, identifying a process, and killing a tree.

use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

#[cfg(windows)]
mod win {
    use core::ffi::c_void;
    pub type Handle = *mut c_void;

    #[repr(C)]
    pub struct StartupInfoW {
        pub cb: u32,
        pub reserved: *mut u16,
        pub desktop: *mut u16,
        pub title: *mut u16,
        pub x: u32,
        pub y: u32,
        pub x_size: u32,
        pub y_size: u32,
        pub x_count_chars: u32,
        pub y_count_chars: u32,
        pub fill_attribute: u32,
        pub flags: u32,
        pub show_window: u16,
        pub reserved2_len: u16,
        pub reserved2: *mut u8,
        pub std_input: Handle,
        pub std_output: Handle,
        pub std_error: Handle,
    }

    #[repr(C)]
    pub struct ProcessInformation {
        pub process: Handle,
        pub thread: Handle,
        pub process_id: u32,
        pub thread_id: u32,
    }

    extern "system" {
        #[allow(clippy::too_many_arguments)]
        pub fn CreateProcessW(
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> i32;
        pub fn CloseHandle(h: Handle) -> i32;
        pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        pub fn GetProcessTimes(
            h: Handle,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }

    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    pub const PROCESS_SET_QUOTA: u32 = 0x0100;
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    pub const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// `JobObjectExtendedLimitInformation`.
    pub const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

    #[repr(C)]
    #[derive(Default)]
    pub struct BasicLimitInformation {
        pub per_process_user_time_limit: i64,
        pub per_job_user_time_limit: i64,
        pub limit_flags: u32,
        pub minimum_working_set_size: usize,
        pub maximum_working_set_size: usize,
        pub active_process_limit: u32,
        pub affinity: usize,
        pub priority_class: u32,
        pub scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct ExtendedLimitInformation {
        pub basic: BasicLimitInformation,
        pub io_counters: [u64; 6],
        pub process_memory_limit: usize,
        pub job_memory_limit: usize,
        pub peak_process_memory_used: usize,
        pub peak_job_memory_used: usize,
    }

    #[link(name = "advapi32")]
    extern "system" {
        pub fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        pub fn DuplicateTokenEx(
            token: Handle,
            access: u32,
            attributes: *const c_void,
            impersonation_level: i32,
            token_type: i32,
            new_token: *mut Handle,
        ) -> i32;
        pub fn GetTokenInformation(
            token: Handle,
            class: i32,
            info: *mut c_void,
            len: u32,
            returned: *mut u32,
        ) -> i32;
        pub fn SetTokenInformation(token: Handle, class: i32, info: *const c_void, len: u32)
            -> i32;
        #[allow(clippy::too_many_arguments)]
        pub fn CreateProcessAsUserW(
            token: Handle,
            application: *const u16,
            command_line: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit_handles: i32,
            creation_flags: u32,
            environment: *const c_void,
            current_directory: *const u16,
            startup_info: *const StartupInfoW,
            process_information: *mut ProcessInformation,
        ) -> i32;
        pub fn GetSidSubAuthorityCount(sid: *const c_void) -> *const u8;
        pub fn GetSidSubAuthority(sid: *const c_void, index: u32) -> *const u32;
        pub fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }
    extern "system" {
        pub fn GetCurrentProcess() -> Handle;
    }
    #[link(name = "iphlpapi")]
    extern "system" {
        pub fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            order: i32,
            family: u32,
            class: i32,
            reserved: u32,
        ) -> u32;
    }
    pub const AF_INET: u32 = 2;
    /// `TCP_TABLE_OWNER_PID_LISTENER`.
    pub const TCP_TABLE_OWNER_PID_LISTENER: i32 = 3;

    pub const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
    pub const TOKEN_DUPLICATE: u32 = 0x0002;
    pub const TOKEN_QUERY: u32 = 0x0008;
    pub const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
    pub const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
    pub const SECURITY_IMPERSONATION: i32 = 2;
    pub const TOKEN_PRIMARY: i32 = 1;
    /// `TokenIntegrityLevel`.
    pub const TOKEN_INTEGRITY_LEVEL: i32 = 25;
    pub const SE_GROUP_INTEGRITY: u32 = 0x20;
    /// `S-1-16-8192`, the medium mandatory level, as a SID in memory: revision 1, one
    /// sub-authority, authority 16, sub-authority 0x2000.
    pub static MEDIUM_SID: [u32; 3] = [0x0000_0101, 0x1000_0000, 0x2000];
    pub const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    pub const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    pub const RRF_RT_REG_SZ: u32 = 0x2;

    #[repr(C)]
    pub struct TokenMandatoryLabel {
        pub sid: *const c_void,
        pub attributes: u32,
    }

    extern "system" {
        pub fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        pub fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *const c_void,
            len: u32,
        ) -> i32;
        pub fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        pub fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
    }
}

/// A Windows job whose processes all end when it is terminated or its last handle closes.
#[cfg(windows)]
pub struct Job(win::Handle);

// SAFETY: a job handle is a kernel handle, usable from any thread.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// A new job that kills its processes when its last handle closes.
    pub fn kill_on_close() -> Option<Job> {
        // SAFETY: the job handle is checked before use; `info` is a valid, fully initialised
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION passed with its exact size.
        unsafe {
            let job = win::CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info = win::ExtendedLimitInformation::default();
            info.basic.limit_flags = win::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = win::SetInformationJobObject(
                job,
                win::JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<win::ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                win::CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }

    /// Puts process `pid` (and the processes it starts from now on) in the job.
    pub fn assign(&self, pid: u32) -> bool {
        // SAFETY: the process handle is checked before use and closed after.
        unsafe {
            let h = win::OpenProcess(win::PROCESS_SET_QUOTA | win::PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                return false;
            }
            let ok = win::AssignProcessToJobObject(self.0, h);
            win::CloseHandle(h);
            ok != 0
        }
    }

    /// Ends every process in the job.
    pub fn terminate(&self) -> bool {
        // SAFETY: `self.0` is a live job handle owned by this value.
        unsafe { win::TerminateJobObject(self.0, 1) != 0 }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a live job handle owned by this value, closed once.
        unsafe {
            win::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
pub struct Job;

#[cfg(not(windows))]
impl Job {
    pub fn kill_on_close() -> Option<Job> {
        None
    }
    pub fn assign(&self, _pid: u32) -> bool {
        false
    }
    pub fn terminate(&self) -> bool {
        false
    }
}

/// When process `pid` started, as a Windows FILETIME, if it is running and readable.
#[cfg(windows)]
pub fn process_start_time(pid: u32) -> Option<u64> {
    // SAFETY: the handle is checked before use and closed after; the out-pointers are valid locals.
    unsafe {
        let h = win::OpenProcess(win::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
        let ok = win::GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user);
        win::CloseHandle(h);
        (ok != 0).then_some(created)
    }
}

#[cfg(not(windows))]
pub fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

/// The mandatory integrity level of process `pid`, or of this process when `None`: 0x2000 is
/// medium, 0x3000 high.
#[cfg(windows)]
pub fn integrity_level(pid: Option<u32>) -> Option<u32> {
    // SAFETY: every handle is checked before use and closed after; the buffer outlives the reads,
    // and the SID read from it is the one GetTokenInformation wrote there.
    unsafe {
        let process = match pid {
            Some(pid) => win::OpenProcess(win::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid),
            None => win::GetCurrentProcess(),
        };
        if process.is_null() {
            return None;
        }
        let mut token = std::ptr::null_mut();
        let opened = win::OpenProcessToken(process, win::TOKEN_QUERY, &mut token);
        if pid.is_some() {
            win::CloseHandle(process);
        }
        if opened == 0 {
            return None;
        }
        let mut buf = [0u64; 16];
        let mut len = 0;
        let ok = win::GetTokenInformation(
            token,
            win::TOKEN_INTEGRITY_LEVEL,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        );
        win::CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let sid = (*(buf.as_ptr() as *const win::TokenMandatoryLabel)).sid;
        let count = *win::GetSidSubAuthorityCount(sid);
        Some(*win::GetSidSubAuthority(sid, count.checked_sub(1)? as u32))
    }
}

#[cfg(not(windows))]
pub fn integrity_level(_pid: Option<u32>) -> Option<u32> {
    None
}

/// Whether this process runs at high integrity or above.
pub fn elevated() -> bool {
    integrity_level(None).is_some_and(|il| il >= 0x3000)
}

/// The process listening on loopback TCP port `port`, as WebView2's browser process listens on
/// the debugging port.
#[cfg(windows)]
pub fn listening_pid(port: u16) -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: a null table with a zero size asks only for the size needed.
    unsafe {
        win::GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            win::AF_INET,
            win::TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    let mut buf = vec![0u32; size as usize / 4 + 1];
    let mut size = (buf.len() * 4) as u32;
    // SAFETY: `buf` is a writable, 4-byte aligned buffer of `size` bytes.
    let status = unsafe {
        win::GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
            0,
            win::AF_INET,
            win::TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if status != 0 {
        return None;
    }
    // MIB_TCPTABLE_OWNER_PID: a count, then rows of state, local address, local port (network
    // order in the low 16 bits), remote address, remote port and owning pid.
    let rows = buf.get(1..1 + 6 * *buf.first()? as usize)?;
    rows.as_chunks::<6>()
        .0
        .iter()
        .find(|row| u16::from_be(row[2] as u16) == port)
        .map(|row| row[5])
}

#[cfg(not(windows))]
pub fn listening_pid(_port: u16) -> Option<u32> {
    None
}

/// The version of the Evergreen WebView2 runtime installed for the machine or for this user,
/// the newer of the two.
#[cfg(windows)]
pub fn webview2_runtime() -> Option<String> {
    const CLIENT: &str = "Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let read = |root: isize, path: String| -> Option<String> {
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = "pv".encode_utf16().chain(Some(0)).collect();
        let mut buf = [0u16; 64];
        let mut len = std::mem::size_of_val(&buf) as u32;
        // SAFETY: both names are NUL-terminated; `buf` and `len` describe a writable buffer.
        let status = unsafe {
            win::RegGetValueW(
                root,
                path.as_ptr(),
                name.as_ptr(),
                win::RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                &mut len,
            )
        };
        let text = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
        (status == 0 && runtime_major(&text).is_some()).then_some(text)
    };
    [
        read(
            win::HKEY_LOCAL_MACHINE,
            format!("SOFTWARE\\WOW6432Node\\{CLIENT}"),
        ),
        read(win::HKEY_CURRENT_USER, format!("Software\\{CLIENT}")),
    ]
    .into_iter()
    .flatten()
    .max_by_key(|v| runtime_major(v))
}

#[cfg(not(windows))]
pub fn webview2_runtime() -> Option<String> {
    None
}

/// The major version in a version string such as `156.0.4314.42`.
pub fn runtime_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Whether a session launching an app must start at medium integrity for WebView2 to open the
/// debugging port: this process is elevated and the runtime is 150 or later, or unknown.
pub fn launch_needs_medium(elevated: bool, runtime: Option<&str>) -> bool {
    elevated
        && runtime
            .and_then(runtime_major)
            .is_none_or(|major| major >= 150)
}

/// A primary token like this process's, at medium integrity.
#[cfg(windows)]
fn medium_token() -> std::io::Result<win::Handle> {
    // SAFETY: each handle is checked before use; the label points at a static SID and is passed
    // with the size of the label plus that SID.
    unsafe {
        let mut own = std::ptr::null_mut();
        let access = win::TOKEN_DUPLICATE
            | win::TOKEN_QUERY
            | win::TOKEN_ADJUST_DEFAULT
            | win::TOKEN_ASSIGN_PRIMARY;
        if win::OpenProcessToken(win::GetCurrentProcess(), access, &mut own) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut token = std::ptr::null_mut();
        let duplicated = win::DuplicateTokenEx(
            own,
            win::MAXIMUM_ALLOWED,
            std::ptr::null(),
            win::SECURITY_IMPERSONATION,
            win::TOKEN_PRIMARY,
            &mut token,
        );
        win::CloseHandle(own);
        if duplicated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let label = win::TokenMandatoryLabel {
            sid: win::MEDIUM_SID.as_ptr() as *const core::ffi::c_void,
            attributes: win::SE_GROUP_INTEGRITY,
        };
        let len = std::mem::size_of::<win::TokenMandatoryLabel>()
            + std::mem::size_of_val(&win::MEDIUM_SID);
        if win::SetTokenInformation(
            token,
            win::TOKEN_INTEGRITY_LEVEL,
            &label as *const _ as *const core::ffi::c_void,
            len as u32,
        ) == 0
        {
            let e = std::io::Error::last_os_error();
            win::CloseHandle(token);
            return Err(e);
        }
        Ok(token)
    }
}

/// Starts `exe args…` detached from this console and, where the job allows it, from this job,
/// inheriting no handles; at medium integrity when `at_medium`. Returns the child's process id.
#[cfg(windows)]
pub fn spawn_detached(
    exe: &std::path::Path,
    args: &[&str],
    at_medium: bool,
) -> std::io::Result<u32> {
    let token = if at_medium {
        Some(medium_token()?)
    } else {
        None
    };
    let quote = |s: &str| {
        if s.contains(' ') || s.is_empty() {
            format!("\"{s}\"")
        } else {
            s.to_string()
        }
    };
    let mut line = quote(&exe.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote(a));
    }
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let mut last = std::io::Error::other("CreateProcessW was not called");
    for flags in [base | CREATE_BREAKAWAY_FROM_JOB, base] {
        let mut wide: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
        // SAFETY: an all-zero STARTUPINFOW with only `cb` set is valid input; the command line is a
        // writable NUL-terminated buffer, as CreateProcessW and CreateProcessAsUserW require, and
        // outlives the call; `token` is a live primary token.
        unsafe {
            let mut si: win::StartupInfoW = std::mem::zeroed();
            si.cb = std::mem::size_of::<win::StartupInfoW>() as u32;
            let mut pi: win::ProcessInformation = std::mem::zeroed();
            let ok = match token {
                Some(token) => win::CreateProcessAsUserW(
                    token,
                    std::ptr::null(),
                    wide.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    std::ptr::null(),
                    &si,
                    &mut pi,
                ),
                None => win::CreateProcessW(
                    std::ptr::null(),
                    wide.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    std::ptr::null(),
                    &si,
                    &mut pi,
                ),
            };
            if ok != 0 {
                win::CloseHandle(pi.thread);
                win::CloseHandle(pi.process);
                if let Some(token) = token {
                    win::CloseHandle(token);
                }
                return Ok(pi.process_id);
            }
            last = std::io::Error::last_os_error();
        }
    }
    if let Some(token) = token {
        // SAFETY: a live token handle owned here, closed once.
        unsafe {
            win::CloseHandle(token);
        }
    }
    Err(last)
}

#[cfg(not(windows))]
pub fn spawn_detached(
    exe: &std::path::Path,
    args: &[&str],
    _at_medium: bool,
) -> std::io::Result<u32> {
    use std::process::Stdio;
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|c| c.id())
}

/// The foreground window, as a number (0 when there is none), and the process that owns it.
#[cfg(windows)]
pub fn foreground() -> (isize, u32) {
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> *mut core::ffi::c_void;
        fn GetWindowThreadProcessId(hwnd: *mut core::ffi::c_void, pid: *mut u32) -> u32;
    }
    // SAFETY: both take plain values; `pid` is a valid out-pointer.
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (hwnd as isize, pid)
    }
}

/// Brings `hwnd` to the foreground, where Windows lets this process; whether it did.
#[cfg(windows)]
pub fn set_foreground(hwnd: isize) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn SetForegroundWindow(hwnd: *mut core::ffi::c_void) -> i32;
    }
    // SAFETY: an invalid handle makes this fail, not misbehave.
    unsafe { SetForegroundWindow(hwnd as *mut core::ffi::c_void) != 0 }
}

/// The image name of process `pid`, if it is running.
#[cfg(windows)]
pub fn image_name(pid: u32) -> Option<String> {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains(&format!("\"{pid}\"")))?;
    line.split(',')
        .next()
        .map(|s| s.trim_matches('"').to_string())
}

#[cfg(not(windows))]
pub fn image_name(_pid: u32) -> Option<String> {
    None
}

/// Kills process `pid` and its children; describes how that went.
pub fn kill_tree(pid: u32) -> String {
    let start = std::time::Instant::now();
    #[cfg(windows)]
    let out = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    #[cfg(not(windows))]
    let out = Command::new("kill").args(["-9", &pid.to_string()]).output();
    match out {
        Ok(o) => format!(
            "exit {:?} in {} ms: {}",
            o.status.code(),
            start.elapsed().as_millis(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => format!("did not run: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_needs_medium_only_when_elevated_on_runtime_150_or_later_or_an_unknown_one() {
        assert!(launch_needs_medium(true, Some("150.0.4078.48")));
        assert!(launch_needs_medium(true, Some("156.0.4314.42")));
        assert!(launch_needs_medium(true, None));
        assert!(!launch_needs_medium(true, Some("149.0.4022.98")));
        assert!(!launch_needs_medium(true, Some("131.0.2903.86")));
        assert!(!launch_needs_medium(false, Some("156.0.4314.42")));
        assert!(!launch_needs_medium(false, None));
    }

    #[cfg(windows)]
    #[test]
    fn the_process_listening_on_a_port_is_found_by_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(listening_pid(port), Some(std::process::id()));
        drop(listener);
        assert_eq!(listening_pid(port), None);
    }

    #[cfg(windows)]
    #[test]
    fn this_process_has_an_integrity_level_and_a_medium_token_can_be_made() {
        let own = integrity_level(None).unwrap();
        assert!(own >= 0x2000, "{own:#x}");
        let token = medium_token().unwrap();
        // SAFETY: a live token handle returned above, closed once.
        unsafe {
            win::CloseHandle(token);
        }
    }
}

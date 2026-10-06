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

/// Starts `exe args…` detached from this console and, where the job allows it, from this job,
/// inheriting no handles. Returns the child's process id.
#[cfg(windows)]
pub fn spawn_detached(exe: &std::path::Path, args: &[&str]) -> std::io::Result<u32> {
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
        // writable NUL-terminated buffer, as CreateProcessW requires, and outlives the call.
        unsafe {
            let mut si: win::StartupInfoW = std::mem::zeroed();
            si.cb = std::mem::size_of::<win::StartupInfoW>() as u32;
            let mut pi: win::ProcessInformation = std::mem::zeroed();
            let ok = win::CreateProcessW(
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
            );
            if ok != 0 {
                win::CloseHandle(pi.thread);
                win::CloseHandle(pi.process);
                return Ok(pi.process_id);
            }
            last = std::io::Error::last_os_error();
        }
    }
    Err(last)
}

#[cfg(not(windows))]
pub fn spawn_detached(exe: &std::path::Path, args: &[&str]) -> std::io::Result<u32> {
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

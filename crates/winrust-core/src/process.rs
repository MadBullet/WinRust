//! Process enumeration, inspection and control.
//!
//! All functions work with the *least* privilege that still gives a useful
//! answer: `PROCESS_QUERY_LIMITED_INFORMATION` succeeds for most processes even
//! when running as a standard user, and only falls back to nothing when Windows
//! categorically refuses (protected processes, other sessions).

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::wide::{from_wide_fixed, from_wide_len};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{FILETIME, MAX_PATH};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetPriorityClass, GetProcessHandleCount, GetProcessIoCounters,
    GetProcessTimes, OpenProcess, OpenThread, QueryFullProcessImageNameW,
    ResumeThread, SetPriorityClass, SetProcessAffinityMask, SuspendThread,
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, IO_COUNTERS, NORMAL_PRIORITY_CLASS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE, PROCESS_VM_READ, REALTIME_PRIORITY_CLASS,
    THREAD_SUSPEND_RESUME,
};

/// One row of the Task Manager process list, as far as Task Manager shows it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    /// Image base name, e.g. `explorer.exe`.
    pub name: String,
    /// Full image path, empty when access is denied.
    pub exe_path: String,
    pub threads: u32,
    /// Working set (private + shared resident) in bytes.
    pub working_set: u64,
    /// Private commit in bytes (what Task Manager's "Memory" column shows).
    pub private_bytes: u64,
    /// Kernel + user CPU time over the process lifetime, in 100 ns units.
    pub cpu_time_100ns: u64,
    /// Creation time in 100 ns units since 1601-01-01 (stable identity).
    pub created_100ns: u64,
    pub priority: Priority,
    /// True when the process is currently suspended by WinRust.
    pub suspended: bool,
    /// Open handles, as shown by Resource Monitor (0 when inaccessible).
    pub handles: u32,
    /// Cumulative bytes read from storage (I/O counters).
    pub io_read_bytes: u64,
    /// Cumulative bytes written to storage.
    pub io_write_bytes: u64,
    /// Cumulative bytes read/written by other (non-file) I/O operations.
    pub io_other_bytes: u64,
    /// Access we managed to obtain: `full`, `limited`, `denied`.
    pub access: AccessLevel,
    /// Owning account (`DOMAIN\\user`), empty when the token is inaccessible.
    /// The Details tab shows this and Task Manager sorts by it.
    pub user_name: String,
    /// Session the process belongs to (0 = services, 1+ = interactive).
    pub session_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AccessLevel {
    #[default]
    Denied,
    Limited,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Priority {
    Idle,
    BelowNormal,
    #[default]
    Normal,
    AboveNormal,
    High,
    Realtime,
    Unknown,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Idle => crate::i18n::ts("Low"),
            Priority::BelowNormal => crate::i18n::ts("Below normal"),
            Priority::Normal => crate::i18n::ts("Normal"),
            Priority::AboveNormal => crate::i18n::ts("Above normal"),
            Priority::High => crate::i18n::ts("High"),
            Priority::Realtime => crate::i18n::ts("Realtime"),
            Priority::Unknown => "—",
        }
    }

    pub fn flag(self) -> u32 {
        match self {
            Priority::Idle => IDLE_PRIORITY_CLASS,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Priority::High => HIGH_PRIORITY_CLASS,
            Priority::Realtime => REALTIME_PRIORITY_CLASS,
            Priority::Unknown => NORMAL_PRIORITY_CLASS,
        }
    }
}

pub const ALL_PRIORITIES: [Priority; 6] = [
    Priority::Idle,
    Priority::BelowNormal,
    Priority::Normal,
    Priority::AboveNormal,
    Priority::High,
    Priority::Realtime,
];

fn filetime_to_u64(ft: &FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// Enumerate every process visible to the current user.
///
/// The Toolhelp snapshot lists all PIDs (even protected ones), so the list is
/// always complete; per-process details are filled in where access allows.
pub fn enumerate() -> Result<Vec<ProcessInfo>> {
    let snap = Handle::from_call(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) },
        "CreateToolhelp32Snapshot",
    )?;

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    let mut out = Vec::with_capacity(256);
    let mut ok = unsafe { Process32FirstW(snap.raw(), &mut entry) };
    while ok != 0 {
        let name = from_wide_fixed(&entry.szExeFile);
        let mut info = ProcessInfo {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            name,
            threads: entry.cntThreads,
            ..Default::default()
        };
        enrich(&mut info);
        out.push(info);
        ok = unsafe { Process32NextW(snap.raw(), &mut entry) };
    }

    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    Ok(out)
}

/// Fill path/CPU/memory/priority for one process.  Never fails: partial data
/// is better than an invisible row.
fn enrich(info: &mut ProcessInfo) {
    let pid = info.pid;
    if let Some(h) = open_limited(pid) {
        info.access = AccessLevel::Limited;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(h.raw(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        };
        if ok != 0 {
            info.exe_path = crate::wide::from_wide(&buf, len as usize);
            info.access = AccessLevel::Full;
        }

        let mut creation = unsafe { std::mem::zeroed::<FILETIME>() };
        let mut exit = unsafe { std::mem::zeroed::<FILETIME>() };
        let mut kernel = unsafe { std::mem::zeroed::<FILETIME>() };
        let mut user = unsafe { std::mem::zeroed::<FILETIME>() };
        if unsafe { GetProcessTimes(h.raw(), &mut creation, &mut exit, &mut kernel, &mut user) }
            != 0
        {
            info.created_100ns = filetime_to_u64(&creation);
            info.cpu_time_100ns = filetime_to_u64(&kernel) + filetime_to_u64(&user);
        }

        let mut mem: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if unsafe { K32GetProcessMemoryInfo(h.raw(), &mut mem as *mut _ as *mut _, mem.cb) } != 0 {
            info.working_set = mem.WorkingSetSize as u64;
            info.private_bytes = mem.PrivateUsage as u64;
        }

        let mut io: IO_COUNTERS = unsafe { std::mem::zeroed() };
        if unsafe { GetProcessIoCounters(h.raw(), &mut io) } != 0 {
            info.io_read_bytes = io.ReadTransferCount;
            info.io_write_bytes = io.WriteTransferCount;
            info.io_other_bytes = io.OtherTransferCount;
        }

        let mut handles = 0u32;
        if unsafe { GetProcessHandleCount(h.raw(), &mut handles) } != 0 {
            info.handles = handles;
        }

        info.user_name = process_user(pid);
        info.session_id = unsafe {
            let mut sid = 0u32;
            if ProcessIdToSessionId(pid, &mut sid) != 0 {
                sid
            } else {
                0
            }
        };

        let prio = unsafe { GetPriorityClass(h.raw()) };
        info.priority = match prio {
            IDLE_PRIORITY_CLASS => Priority::Idle,
            BELOW_NORMAL_PRIORITY_CLASS => Priority::BelowNormal,
            NORMAL_PRIORITY_CLASS => Priority::Normal,
            ABOVE_NORMAL_PRIORITY_CLASS => Priority::AboveNormal,
            HIGH_PRIORITY_CLASS => Priority::High,
            REALTIME_PRIORITY_CLASS => Priority::Realtime,
            _ => Priority::Unknown,
        };

        let mut exit_code = 0u32;
        if unsafe { GetExitCodeProcess(h.raw(), &mut exit_code) } == 0 {
            info.access = AccessLevel::Denied;
        }
    } else if open_full(pid).is_some() {
        info.access = AccessLevel::Full;
    } else {
        info.access = AccessLevel::Denied;
    }
}

/// Cache of SID string -> display name, so the 300-process Details tab does not
/// call `LookupAccountSidW` 300 times a second.  Only a handful of distinct
/// accounts exist on a normal machine.
fn user_cache() -> &'static std::sync::Mutex<std::collections::HashMap<Vec<u8>, String>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<Vec<u8>, String>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// `DOMAIN\\user` for a process, by way of its token.  Empty when the token is
/// inaccessible, which is normal for protected processes - the Details tab then
/// shows an em dash, exactly like Task Manager.
fn process_user(pid: u32) -> String {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::OpenProcessToken;

    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return String::new();
    }
    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe { OpenProcessToken(h, TOKEN_QUERY, &mut token) } != 0;
    unsafe { CloseHandle(h) };
    if !opened {
        return String::new();
    }

    // Two-step: ask for the size, then read the SID out of the buffer.
    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        unsafe { CloseHandle(token) };
        return String::new();
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut _,
            needed,
            &mut needed,
        )
    } != 0;
    unsafe { CloseHandle(token) };
    if !ok {
        return String::new();
    }
    let user: TOKEN_USER = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const TOKEN_USER) };
    let sid = user.User.Sid;
    if sid.is_null() {
        return String::new();
    }

    // Cache by the SID bytes.  `GetLengthSid` is not in scope, so read the
    // sub-authority count out of the SID header: 8 bytes + 4 per authority.
    let sid_len = unsafe {
        let revision = *(sid as *const u8) as usize;
        let _ = revision;
        let count = *((sid as *const u8).add(1)) as usize;
        8 + 4 * count
    };
    let key = unsafe { std::slice::from_raw_parts(sid as *const u8, sid_len).to_vec() };
    if let Ok(cache) = user_cache().lock() {
        if let Some(name) = cache.get(&key) {
            return name.clone();
        }
    }

    let mut name = [0u16; 256];
    let mut name_len = name.len() as u32;
    let mut domain = [0u16; 256];
    let mut domain_len = domain.len() as u32;
    let mut use_kind = 0i32;
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_kind,
        )
    } != 0;
    let result = if ok {
        let account = from_wide_len(&name, name_len as usize);
        let dom = from_wide_len(&domain, domain_len as usize);
        if dom.is_empty() {
            account
        } else {
            format!("{dom}\\{account}")
        }
    } else {
        String::new()
    };
    if let Ok(mut cache) = user_cache().lock() {
        cache.insert(key, result.clone());
    }
    result
}

fn open_limited(pid: u32) -> Option<Handle> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    Handle::new(h)
}

fn open_full(pid: u32) -> Option<Handle> {
    let h = unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION
                | PROCESS_VM_READ
                | PROCESS_TERMINATE
                | PROCESS_SET_INFORMATION,
            0,
            pid,
        )
    };
    Handle::new(h)
}

fn open_with(pid: u32, access: u32) -> Result<Handle> {
    Handle::from_call(unsafe { OpenProcess(access, 0, pid) }, "OpenProcess")
}

/// Terminate a process (Task Manager's "End task").
pub fn terminate(pid: u32) -> Result<()> {
    let h = open_with(pid, PROCESS_TERMINATE)?;
    check("TerminateProcess", unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(h.raw(), 1)
    })
}

/// Change a process priority class.
pub fn set_priority(pid: u32, priority: Priority) -> Result<()> {
    let h = open_with(pid, PROCESS_SET_INFORMATION)?;
    check("SetPriorityClass", unsafe {
        SetPriorityClass(h.raw(), priority.flag())
    })
}

/// Suspend every thread of a process (a WinRust extension over Task Manager).
pub fn suspend(pid: u32) -> Result<usize> {
    let h = open_with(pid, PROCESS_SUSPEND_RESUME)?;
    let n = unsafe {
        windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(
            4, /* TH32CS_SNAPTHREAD */
            0,
        )
    };
    let snap = match Handle::new(n) {
        Some(s) => s,
        None => {
            return Err(WinError::from_last_error(
                "CreateToolhelp32Snapshot(threads)",
            ))
        }
    };
    let mut te: windows_sys::Win32::System::Diagnostics::ToolHelp::THREADENTRY32 =
        unsafe { std::mem::zeroed() };
    te.dwSize = std::mem::size_of_val(&te) as u32;
    let mut count = 0;
    let mut ok = unsafe {
        windows_sys::Win32::System::Diagnostics::ToolHelp::Thread32First(snap.raw(), &mut te)
    };
    while ok != 0 {
        if te.th32OwnerProcessID == pid {
            let th = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID) };
            if let Some(th) = Handle::new(th) {
                unsafe { SuspendThread(th.raw()) };
                count += 1;
            }
        }
        ok = unsafe {
            windows_sys::Win32::System::Diagnostics::ToolHelp::Thread32Next(snap.raw(), &mut te)
        };
    }
    let _ = h;
    Ok(count)
}

/// Resume every thread of a process.
pub fn resume(pid: u32) -> Result<usize> {
    let h = open_with(pid, PROCESS_SUSPEND_RESUME)?;
    let n = unsafe {
        windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(4, 0)
    };
    let snap = match Handle::new(n) {
        Some(s) => s,
        None => {
            return Err(WinError::from_last_error(
                "CreateToolhelp32Snapshot(threads)",
            ))
        }
    };
    let mut te: windows_sys::Win32::System::Diagnostics::ToolHelp::THREADENTRY32 =
        unsafe { std::mem::zeroed() };
    te.dwSize = std::mem::size_of_val(&te) as u32;
    let mut count = 0;
    let mut ok = unsafe {
        windows_sys::Win32::System::Diagnostics::ToolHelp::Thread32First(snap.raw(), &mut te)
    };
    while ok != 0 {
        if te.th32OwnerProcessID == pid {
            let th = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID) };
            if let Some(th) = Handle::new(th) {
                unsafe { ResumeThread(th.raw()) };
                count += 1;
            }
        }
        ok = unsafe {
            windows_sys::Win32::System::Diagnostics::ToolHelp::Thread32Next(snap.raw(), &mut te)
        };
    }
    let _ = h;
    Ok(count)
}

/// Bind a process to the given CPU cores (bit mask).  `0` means all cores.
pub fn set_affinity(pid: u32, mask: usize) -> Result<()> {
    let h = open_with(pid, PROCESS_SET_INFORMATION)?;
    let cores = crate::system::cpu_count();
    let mask = if mask == 0 {
        (1usize << cores.min(64)) - 1
    } else {
        mask
    };
    check("SetProcessAffinityMask", unsafe {
        SetProcessAffinityMask(h.raw(), mask)
    })
}

/// Current affinity mask of a process.
pub fn affinity(pid: u32) -> Result<(usize, usize)> {
    let h = open_with(pid, PROCESS_QUERY_INFORMATION)?;
    let mut proc_mask = 0usize;
    let mut sys_mask = 0usize;
    check("GetProcessAffinityMask", unsafe {
        windows_sys::Win32::System::Threading::GetProcessAffinityMask(
            h.raw(),
            &mut proc_mask,
            &mut sys_mask,
        )
    })?;
    Ok((proc_mask, sys_mask))
}

/// The file name component of a path, e.g. `C:\x\explorer.exe` -> `explorer.exe`.
pub fn basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Wait until a process exits.  Returns the exit code.
pub fn wait_for_exit(pid: u32, timeout_ms: u32) -> Result<u32> {
    let h = open_with(
        pid,
        windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE
            | PROCESS_QUERY_LIMITED_INFORMATION,
    )?;
    let r =
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(h.raw(), timeout_ms) };
    if r != windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
        return Err(WinError::Other(format!("wait timed out or failed: {r}")));
    }
    let mut code = 0u32;
    check("GetExitCodeProcess", unsafe {
        GetExitCodeProcess(h.raw(), &mut code)
    })?;
    Ok(code)
}

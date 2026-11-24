//! Machine-level information: memory, uptime, CPU, OS build, computer name.

use crate::error::{check, Result, WinError};
use crate::wide::from_wide;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::{
    GetComputerNameExW, GetNativeSystemInfo, GetPhysicallyInstalledSystemMemory, GetTickCount64,
    GlobalMemoryStatusEx, COMPUTER_NAME_FORMAT, MEMORYSTATUSEX, SYSTEM_INFO,
    ComputerNameDnsHostname, ComputerNamePhysicalDnsDomain,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// Live memory counters, refreshed by the UI on every frame tick.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MemoryStatus {
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
    pub total_virtual: u64,
    pub avail_virtual: u64,
    /// Windows' own "Memory in use" percentage.
    pub load_percent: u32,
}

impl MemoryStatus {
    pub fn used_phys(&self) -> u64 {
        self.total_phys.saturating_sub(self.avail_phys)
    }

    pub fn used_percent(&self) -> f32 {
        if self.total_phys == 0 {
            0.0
        } else {
            (self.used_phys() as f64 / self.total_phys as f64 * 100.0) as f32
        }
    }
}

pub fn memory_status() -> MemoryStatus {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return MemoryStatus::default();
    }
    MemoryStatus {
        total_phys: m.ullTotalPhys,
        avail_phys: m.ullAvailPhys,
        total_page_file: m.ullTotalPageFile,
        avail_page_file: m.ullAvailPageFile,
        total_virtual: m.ullTotalVirtual,
        avail_virtual: m.ullAvailVirtual,
        load_percent: m.dwMemoryLoad,
    }
}

/// System-wide CPU usage sampled between two calls, in `0..=100`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuSample {
    idle: u64,
    kernel: u64,
    user: u64,
}

fn ft64(ft: &FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

pub fn cpu_sample() -> CpuSample {
    let mut idle: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    unsafe {
        GetSystemTimes(&mut idle, &mut kernel, &mut user);
    }
    CpuSample { idle: ft64(&idle), kernel: ft64(&kernel), user: ft64(&user) }
}

/// CPU usage percentage between two samples (0..=100).
pub fn cpu_usage_between(a: CpuSample, b: CpuSample) -> f32 {
    let idle = b.idle.wrapping_sub(a.idle);
    // GetSystemTimes counts idle inside kernel, so total = kernel + user.
    let total = b.kernel.wrapping_sub(a.kernel) + b.user.wrapping_sub(a.user);
    if total == 0 {
        0.0
    } else {
        let busy = total.saturating_sub(idle);
        (busy as f64 / total as f64 * 100.0) as f32
    }
}

/// Milliseconds since Windows started.
pub fn uptime_ms() -> u64 {
    unsafe { GetTickCount64() }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CpuInfo {
    pub cores: u32,
    pub logical: u32,
    pub arch: &'static str,
    pub page_size: u32,
}

pub fn cpu_info() -> CpuInfo {
    let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetNativeSystemInfo(&mut si) };
    let arch = match unsafe { si.Anonymous.Anonymous.wProcessorArchitecture } {
        0 => "x86",
        5 => "ARM",
        6 => "Itanium",
        9 => "x64",
        12 => "ARM64",
        _ => "unknown",
    };
    CpuInfo {
        cores: si.dwNumberOfProcessors,
        logical: si.dwNumberOfProcessors,
        arch,
        page_size: si.dwPageSize,
    }
}

/// Number of logical processors (used for affinity masks).
pub fn cpu_count() -> usize {
    let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetNativeSystemInfo(&mut si) };
    si.dwNumberOfProcessors.max(1) as usize
}

/// RAM as reported by SMBIOS (bytes), if available.
pub fn installed_ram_bytes() -> Option<u64> {
    let mut kb = 0u64;
    if unsafe { GetPhysicallyInstalledSystemMemory(&mut kb) } != 0 {
        Some(kb * 1024)
    } else {
        None
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OsInfo {
    pub product_name: String,
    pub display_version: String,
    pub build: u32,
    pub ubr: u32,
    pub arch: &'static str,
    pub hostname: String,
    pub domain: String,
}

/// Windows 10/11 marketing name, build number and revision.
///
/// `RtlGetVersion` is used through the PEB (documented enough for this purpose,
/// and the only API that is not shimmed by compatibility layers) so the values
/// match `winver.exe`.
pub fn os_info() -> OsInfo {
    let mut build = 0u32;
    unsafe {
        if let Ok(lib) = module_name("ntdll.dll") {
            if let Some(hm) = non_null_handle(GetModuleHandleW(lib.as_ptr())) {
                if let Some(f) = GetProcAddress(hm, c"RtlGetVersion".as_ptr() as *const u8) {
                    type RtlGetVersion = unsafe extern "system" fn(*mut OsVersionInfoExW) -> i32;
                    let f: RtlGetVersion = std::mem::transmute(f);
                    let mut vi: OsVersionInfoExW = std::mem::zeroed();
                    vi.dw_os_version_info_size = std::mem::size_of::<OsVersionInfoExW>() as u32;
                    if f(&mut vi) == 0 {
                        build = vi.dw_build_number;
                    }
                }
            }
        }
    }
    let product = crate::registry::read_machine_string(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_default();
    let display = crate::registry::read_machine_string(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "DisplayVersion",
    )
    .unwrap_or_default();
    let ubr = crate::registry::read_machine_u32(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "UBR",
    )
    .unwrap_or(0);

    let hostname = computer_name(ComputerNameDnsHostname);
    let domain = computer_name(ComputerNamePhysicalDnsDomain);
    let info = cpu_info();
    OsInfo {
        product_name: product,
        display_version: display,
        build,
        ubr,
        arch: info.arch,
        hostname,
        domain,
    }
}

fn module_name(s: &str) -> Result<Vec<u16>> {
    let w = crate::wide::str_to_wide(s);
    let hm = unsafe { LoadLibraryW(w.as_ptr()) };
    if hm.is_null() {
        return Err(WinError::from_last_error("LoadLibraryW(ntdll)"));
    }
    Ok(w)
}

fn non_null_handle(h: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    if h.is_null() { None } else { Some(h) }
}

#[allow(non_snake_case)]
#[repr(C)]
struct OsVersionInfoExW {
    dw_os_version_info_size: u32,
    dw_major_version: u32,
    dw_minor_version: u32,
    dw_build_number: u32,
    dw_platform_id: u32,
    sz_csd_version: [u16; 128],
    w_service_pack_major: u16,
    w_service_pack_minor: u16,
    w_suite_mask: u16,
    w_product_type: u8,
    w_reserved: u8,
}

pub fn computer_name(kind: COMPUTER_NAME_FORMAT) -> String {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    if unsafe { GetComputerNameExW(kind, buf.as_mut_ptr(), &mut len) } != 0 {
        from_wide(&buf, len as usize)
    } else {
        String::new()
    }
}

/// Friendly summary line such as `Windows 11 Pro 24H2 (26200.7019)`.
pub fn os_summary() -> String {
    let o = os_info();
    let short = if o.build >= 22000 {
        o.product_name.replace("Windows 10", "Windows 11")
    } else {
        o.product_name.clone()
    };
    if o.ubr > 0 {
        format!("{} {} ({}.{})", short, o.display_version, o.build, o.ubr)
    } else {
        format!("{} {}", short, o.display_version)
    }
}

/// Ask the shell to re-broadcast settings changes so apps re-read the
/// environment (used after WinRust edits the user PATH).
pub fn broadcast_settings_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
    let s = crate::wide::str_to_wide("Environment");
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            s.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            3000,
            std::ptr::null_mut(),
        );
    }
}

/// Read a user environment variable (process env is enough for WinRust).
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Raise a process' working set limits (privilege `SeIncreaseWorkingSetPrivilege`).
pub fn set_privilege(name: &str, enable: bool) -> Result<()> {
    use windows_sys::Win32::Security::{LookupPrivilegeValueW, TOKEN_ADJUST_PRIVILEGES, TOKEN_QUERY};
    use windows_sys::Win32::Security::{TOKEN_PRIVILEGES, SE_PRIVILEGE_ENABLED};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = std::ptr::null_mut();
    check(
        "OpenProcessToken",
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) },
    )?;
    let token = crate::handle::Handle::new(token).ok_or(WinError::Other("null token".into()))?;

    let mut luid = unsafe { std::mem::zeroed() };
    let wname = crate::wide::str_to_wide(name);
    check(
        "LookupPrivilegeValueW",
        unsafe { LookupPrivilegeValueW(std::ptr::null(), wname.as_ptr(), &mut luid) },
    )?;

    let mut tp: TOKEN_PRIVILEGES = unsafe { std::mem::zeroed() };
    tp.PrivilegeCount = 1;
    tp.Privileges[0].Luid = luid;
    tp.Privileges[0].Attributes = if enable { SE_PRIVILEGE_ENABLED } else { 0 };

    check(
        "AdjustTokenPrivileges",
        unsafe {
            windows_sys::Win32::Security::AdjustTokenPrivileges(
                token.raw(),
                0,
                &tp,
                std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
    )
}

/// True when the machine has a battery (laptop / UPS-backed tablet).
pub fn pnp_battery_present() -> bool {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut status) } != 0 {
        status.BatteryFlag != 128 // 128 == "no system battery"
    } else {
        false
    }
}

/// Seconds since the last user input, as reported by `GetLastInputInfo`.
pub fn idle_seconds() -> Option<u32> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) / 1000)
}

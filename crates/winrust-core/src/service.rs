//! Service Control Manager: enumeration, configuration and control.
//!
//! Mirrors what `services.msc` shows and allows: list every service and driver,
//! read its start type / logon account / description, start / stop / pause /
//! restart, and edit start type or binary path.

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::wide::{from_wide, str_to_wide};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, EnumDependentServicesW,
    EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfig2W,
    QueryServiceConfigW, QueryServiceStatusEx, StartServiceW, ENUM_SERVICE_STATUSW,
    ENUM_SERVICE_TYPE, QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_ACCEPT_PAUSE_CONTINUE,
    SERVICE_ACCEPT_STOP, SERVICE_ACTIVE, SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
    SERVICE_CONFIG_DESCRIPTION, SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_CONTINUE,
    SERVICE_CONTROL_PAUSE, SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO,
    SERVICE_DEMAND_START, SERVICE_DESCRIPTIONW, SERVICE_DISABLED, SERVICE_DRIVER,
    SERVICE_NO_CHANGE, SERVICE_PAUSED, SERVICE_PAUSE_PENDING, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STATE_ALL, SERVICE_STATUS,
    SERVICE_START_PENDING, SERVICE_PAUSE_PENDING, SERVICE_CONTINUE,
    SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_WIN32, SERVICE_CHANGE_CONFIG, SERVICE_ENUMERATE_DEPENDENTS,
};

/// A service's runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown(u32),
}

impl ServiceState {
    pub fn from_raw(v: u32) -> Self {
        match v {
            SERVICE_STOPPED => Self::Stopped,
            SERVICE_START_PENDING => Self::StartPending,
            SERVICE_STOP_PENDING => Self::StopPending,
            SERVICE_RUNNING => Self::Running,
            SERVICE_CONTINUE_PENDING => Self::ContinuePending,
            SERVICE_PAUSE_PENDING => Self::PausePending,
            SERVICE_PAUSED => Self::Paused,
            other => Self::Unknown(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::StartPending => "Starting",
            Self::StopPending => "Stopping",
            Self::Running => "Running",
            Self::ContinuePending => "Resuming",
            Self::PausePending => "Pausing",
            Self::Paused => "Paused",
            Self::Unknown(_) => "Unknown",
        }
    }

    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    pub fn is_busy(self) -> bool {
        matches!(self, Self::StartPending | Self::StopPending | Self::PausePending | Self::ContinuePending)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartType {
    Boot,
    System,
    Auto,
    /// Automatic (Delayed Start) — the default for most Windows services.
    AutoDelayed,
    Manual,
    Disabled,
    Unknown(u32),
}

impl StartType {
    pub fn from_raw(v: u32, delayed: bool) -> Self {
        match v {
            0 => Self::Boot,
            1 => Self::System,
            2 => if delayed { Self::AutoDelayed } else { Self::Auto },
            3 => Self::Manual,
            4 => Self::Disabled,
            other => Self::Unknown(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Boot => "Boot",
            Self::System => "System",
            Self::Auto => "Automatic",
            Self::AutoDelayed => "Automatic (Delayed Start)",
            Self::Manual => "Manual",
            Self::Disabled => "Disabled",
            Self::Unknown(_) => "Unknown",
        }
    }

    /// The value understood by `ChangeServiceConfigW`.
    pub fn raw(self) -> u32 {
        match self {
            Self::Boot => 0,
            Self::System => 1,
            Self::Auto | Self::AutoDelayed => 2,
            Self::Manual => SERVICE_DEMAND_START,
            Self::Disabled => SERVICE_DISABLED,
            Self::Unknown(v) => v,
        }
    }

    /// User-selectable variants for the properties dialog.
    pub const OPTIONS: [StartType; 5] = [
        StartType::Auto,
        StartType::AutoDelayed,
        StartType::Manual,
        StartType::Disabled,
        StartType::Boot,
    ];
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub state: Option<ServiceState>,
    pub start_type: Option<StartType>,
    pub binary_path: String,
    pub logon_account: String,
    pub load_order_group: String,
    pub dependencies: Vec<String>,
    /// PID of the hosting process, when running.
    pub pid: u32,
    /// True for kernel/file-system drivers.
    pub is_driver: bool,
    pub can_stop: bool,
    pub can_pause: bool,
    pub accepts_stop: bool,
}

/// Handle wrapper around the SCM.
pub struct ScManager {
    h: Handle,
}

impl ScManager {
    pub fn connect() -> Result<Self> {
        let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) };
        match Handle::new(h) {
            Some(h) => Ok(Self { h }),
            None => Err(WinError::from_last_error("OpenSCManagerW (elevation required?)")),
        }
    }

    pub fn open_service(&self, name: &str, access: u32) -> Result<ServiceHandle> {
        let w = str_to_wide(name);
        let h = unsafe { OpenServiceW(self.h.raw() as SC_HANDLE, w.as_ptr(), access) };
        match Handle::new(h) {
            Some(h) => Ok(ServiceHandle { h, name: name.to_string() }),
            None => Err(WinError::from_last_error("OpenServiceW")),
        }
    }

    /// All services and drivers, one row per service, with status filled in.
    pub fn enumerate(&self) -> Result<Vec<ServiceEntry>> {
        // Drivers are enumerated separately from Win32 services so the tree can
        // group them like services.msc does.
        let mut out = self.enumerate_filtered(SERVICE_WIN32, false)?;
        if let Ok(drivers) = self.enumerate_filtered(SERVICE_DRIVER, true) {
            out.extend(drivers);
        }
        out.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        Ok(out)
    }

    fn enumerate_filtered(&self, service_type: ENUM_SERVICE_TYPE, is_driver: bool) -> Result<Vec<ServiceEntry>> {
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut resume = 0u32;
        let empty: [u8; 8] = [0; 8];

        // Size query.
        unsafe {
            EnumServicesStatusExW(
                self.h.raw() as SC_HANDLE,
                SC_ENUM_PROCESS_INFO,
                service_type,
                SERVICE_STATE_ALL,
                empty.as_ptr() as *mut u8,
                0,
                &mut needed,
                &mut count,
                &mut resume,
                std::ptr::null(),
            );
        }
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; needed as usize + 4096];
        let mut entries = 0u32;
        resume = 0;
        let ok = unsafe {
            EnumServicesStatusExW(
                self.h.raw() as SC_HANDLE,
                SC_ENUM_PROCESS_INFO,
                service_type,
                SERVICE_STATE_ALL,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut needed,
                &mut entries,
                &mut resume,
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(WinError::from_last_error("EnumServicesStatusExW"));
        }

        let mut out = Vec::with_capacity(entries as usize);
        let base = buf.as_ptr() as *const windows_sys::Win32::System::Services::ENUM_SERVICE_STATUS_PROCESSW;
        for i in 0..entries as usize {
            let e = unsafe { &*base.add(i) };
            let name = ptr_to_string(e.lpServiceName);
            let display = ptr_to_string(e.lpDisplayName);
            let st = e.ServiceStatusProcess;
            let mut entry = ServiceEntry {
                name: name.clone(),
                display_name: display,
                state: Some(ServiceState::from_raw(st.dwCurrentState)),
                pid: st.dwProcessId,
                is_driver,
                accepts_stop: st.dwControlsAccepted & SERVICE_ACCEPT_STOP != 0,
                can_pause: st.dwControlsAccepted & SERVICE_ACCEPT_PAUSE_CONTINUE != 0,
                ..Default::default()
            };
            // Configuration + description, best-effort (needs SERVICE_QUERY_CONFIG).
            if let Ok(svc) = self.open_service(&name, SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_ENUMERATE_DEPENDENTS) {
                if let Some(cfg) = svc.config() {
                    entry.start_type = Some(cfg.0);
                    entry.binary_path = cfg.1;
                    entry.logon_account = cfg.2;
                    entry.load_order_group = cfg.3;
                }
                entry.description = svc.description().unwrap_or_default();
                entry.dependencies = svc.dependencies();
            }
            out.push(entry);
        }
        Ok(out)
    }
}

fn ptr_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
        if len > 16_384 {
            break;
        }
    }
    let s = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf16_lossy(s)
}

/// An opened service.
pub struct ServiceHandle {
    h: Handle,
    pub name: String,
}

impl ServiceHandle {
    pub fn raw(&self) -> SC_HANDLE {
        self.h.raw() as SC_HANDLE
    }

    pub fn status(&self) -> Result<SERVICE_STATUS_PROCESS> {
        let mut buf = [0u8; std::mem::size_of::<SERVICE_STATUS_PROCESS>()];
        let mut needed = 0u32;
        check(
            "QueryServiceStatusEx",
            unsafe {
                QueryServiceStatusEx(
                    self.raw(),
                    SC_STATUS_PROCESS_INFO,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut needed,
                )
            },
        )?;
        Ok(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const SERVICE_STATUS_PROCESS) })
    }

    pub fn state(&self) -> Option<ServiceState> {
        self.status().ok().map(|s| ServiceState::from_raw(s.dwCurrentState))
    }

    /// `(start_type, binary_path, logon_account, load_order_group)`
    fn config(&self) -> Option<(StartType, String, String, String)> {
        let mut needed = 0u32;
        unsafe {
            QueryServiceConfigW(self.raw(), std::ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe {
            QueryServiceConfigW(self.raw(), buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, needed, &mut needed)
        };
        if ok == 0 {
            return None;
        }
        let cfg = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
        let delayed = self.delayed_auto_start().unwrap_or(false);
        Some((
            StartType::from_raw(cfg.dwStartType, delayed),
            ptr_to_string(cfg.lpBinaryPathName),
            ptr_to_string(cfg.lpServiceStartName),
            ptr_to_string(cfg.lpLoadOrderGroup),
        ))
    }

    pub fn start_type(&self) -> Option<StartType> {
        self.config().map(|c| c.0)
    }

    pub fn binary_path(&self) -> Option<String> {
        self.config().map(|c| c.1)
    }

    pub fn delayed_auto_start(&self) -> Option<bool> {
        let mut info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 0 };
        let mut needed = 0u32;
        let ok = unsafe {
            QueryServiceConfig2W(
                self.raw(),
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                &mut info as *mut _ as *mut u8,
                std::mem::size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32,
                &mut needed,
            )
        };
        if ok != 0 { Some(info.fDelayedAutostart != 0) } else { None }
    }

    pub fn description(&self) -> Option<String> {
        let mut needed = 0u32;
        unsafe {
            QueryServiceConfig2W(self.raw(), SERVICE_CONFIG_DESCRIPTION, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed < std::mem::size_of::<SERVICE_DESCRIPTIONW>() as u32 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe {
            QueryServiceConfig2W(
                self.raw(),
                SERVICE_CONFIG_DESCRIPTION,
                buf.as_mut_ptr(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return None;
        }
        let desc = unsafe { &*(buf.as_ptr() as *const SERVICE_DESCRIPTIONW) };
        let s = ptr_to_string(desc.lpDescription);
        if s.is_empty() { None } else { Some(s) }
    }

    /// Services that must be running before this one can start.
    pub fn dependencies(&self) -> Vec<String> {
        let mut needed = 0u32;
        let mut count = 0u32;
        unsafe {
            EnumDependentServicesW(self.raw(), SERVICE_STATE_ALL, std::ptr::null_mut(), 0, &mut needed, &mut count);
        }
        if needed == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe {
            EnumDependentServicesW(
                self.raw(),
                SERVICE_STATE_ALL,
                buf.as_mut_ptr() as *mut ENUM_SERVICE_STATUSW,
                needed,
                &mut needed,
                &mut count,
            )
        };
        if ok == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let base = buf.as_ptr() as *const ENUM_SERVICE_STATUSW;
        for i in 0..count as usize {
            let e = unsafe { &*base.add(i) };
            out.push(ptr_to_string(e.lpServiceName));
        }
        out
    }

    pub fn start(&self) -> Result<()> {
        check("StartServiceW", unsafe { StartServiceW(self.raw(), 0, std::ptr::null()) })
    }

    pub fn stop(&self) -> Result<()> {
        let mut st = SERVICE_STATUS { dwServiceType: 0, dwCurrentState: 0, dwControlsAccepted: 0, dwWin32ExitCode: 0, dwServiceSpecificExitCode: 0, dwCheckPoint: 0, dwWaitHint: 0 };
        check("ControlService(STOP)", unsafe { ControlService(self.raw(), SERVICE_CONTROL_STOP, &mut st) })
    }

    pub fn pause(&self) -> Result<()> {
        let mut st = SERVICE_STATUS { dwServiceType: 0, dwCurrentState: 0, dwControlsAccepted: 0, dwWin32ExitCode: 0, dwServiceSpecificExitCode: 0, dwCheckPoint: 0, dwWaitHint: 0 };
        check("ControlService(PAUSE)", unsafe { ControlService(self.raw(), SERVICE_CONTROL_PAUSE, &mut st) })
    }

    pub fn resume(&self) -> Result<()> {
        let mut st = SERVICE_STATUS { dwServiceType: 0, dwCurrentState: 0, dwControlsAccepted: 0, dwWin32ExitCode: 0, dwServiceSpecificExitCode: 0, dwCheckPoint: 0, dwWaitHint: 0 };
        check("ControlService(CONTINUE)", unsafe { ControlService(self.raw(), SERVICE_CONTROL_CONTINUE, &mut st) })
    }

    /// Change the start type (and optionally the binary path).
    pub fn set_start_type(&self, start: StartType, binary: Option<&str>) -> Result<()> {
        let wbin = binary.map(str_to_wide);
        let rc = unsafe {
            ChangeServiceConfigW(
                self.raw(),
                SERVICE_NO_CHANGE,
                start.raw(),
                SERVICE_NO_CHANGE,
                wbin.as_ref().map(|w| w.as_ptr()).unwrap_or(std::ptr::null()),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if rc == 0 {
            return Err(WinError::from_last_error("ChangeServiceConfigW"));
        }
        // Delayed start is a separate flag.
        let mut delayed = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: if start == StartType::AutoDelayed { 1 } else { 0 } };
        if matches!(start, StartType::Auto | StartType::AutoDelayed) {
            unsafe {
                QueryServiceConfig2W(
                    self.raw(),
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    &mut delayed as *mut _ as *mut u8,
                    std::mem::size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32,
                    std::ptr::null_mut(),
                );
            }
            delayed.fDelayedAutostart = if start == StartType::AutoDelayed { 1 } else { 0 };
            let ok = unsafe {
                windows_sys::Win32::System::Services::ChangeServiceConfig2W(
                    self.raw(),
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    &delayed as *const _ as *const std::ffi::c_void,
                )
            };
            if ok == 0 {
                return Err(WinError::from_last_error("ChangeServiceConfig2W(delayed)"));
            }
        }
        Ok(())
    }

    /// Wait until the service leaves a pending state (max `timeout_ms`).
    pub fn wait_settled(&self, timeout_ms: u64) -> Option<ServiceState> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let st = self.state()?;
            if !st.is_busy() {
                return Some(st);
            }
            if std::time::Instant::now() >= deadline {
                return Some(st);
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
}

/// Start/stop a service by name with a fresh SCM connection (UI convenience).
pub fn control(name: &str, action: ServiceAction) -> Result<()> {
    let scm = ScManager::connect()?;
    let access = match action {
        ServiceAction::Start => SERVICE_START,
        ServiceAction::Stop => SERVICE_STOP,
        ServiceAction::Pause => windows_sys::Win32::System::Services::SERVICE_PAUSE_CONTINUE,
        ServiceAction::Resume => windows_sys::Win32::System::Services::SERVICE_PAUSE_CONTINUE,
    } | SERVICE_QUERY_STATUS;
    let svc = scm.open_service(name, access)?;
    match action {
        ServiceAction::Start => svc.start()?,
        ServiceAction::Stop => svc.stop()?,
        ServiceAction::Pause => svc.pause()?,
        ServiceAction::Resume => svc.resume()?,
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Pause,
    Resume,
}

impl ServiceAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
        }
    }
}

impl ScManager {
    /// Change a service's start type by name.
    pub fn set_start_type(&self, name: &str, start: StartType) -> Result<()> {
        let svc = self.open_service(name, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG)?;
        svc.set_start_type(start, None)
    }
}

//! Shell integration: launching, known folders, icons, recycle bin.

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::wide::{from_wide, str_to_wide, to_wide};
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::path::Path;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    SHFileOperationW, SHGetFileInfoW, SHGetKnownFolderPath, ShellExecuteExW, ShellExecuteW,
    SHFILEINFOW, SHFILEOPSTRUCTW, SHELLEXECUTEINFOW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
    FOF_NOERRORUI, FOF_WANTNUKEWARNING, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOCLOSEPROCESS,
    SEE_MASK_NOASYNC, SHGFI_DISPLAYNAME, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_TYPENAME,
    SHGFI_USEFILEATTRIBUTES,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Well-known folders exposed by the Explorer sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnownFolder {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Profile,
    LocalAppData,
    RoamingAppData,
    ProgramData,
    Windows,
    ProgramFiles,
    ProgramFilesX86,
    Public,
    RecycleBin,
    Computer,
    Network,
}

impl KnownFolder {
    pub fn label(self) -> &'static str {
        match self {
            KnownFolder::Desktop => "Desktop",
            KnownFolder::Downloads => "Downloads",
            KnownFolder::Documents => "Documents",
            KnownFolder::Pictures => "Pictures",
            KnownFolder::Music => "Music",
            KnownFolder::Videos => "Videos",
            KnownFolder::Profile => "Home",
            KnownFolder::LocalAppData => "Local AppData",
            KnownFolder::RoamingAppData => "Roaming AppData",
            KnownFolder::ProgramData => "ProgramData",
            KnownFolder::Windows => "Windows",
            KnownFolder::ProgramFiles => "Program Files",
            KnownFolder::ProgramFilesX86 => "Program Files (x86)",
            KnownFolder::Public => "Public",
            KnownFolder::RecycleBin => "Recycle Bin",
            KnownFolder::Computer => "This PC",
            KnownFolder::Network => "Network",
        }
    }

    fn guid(self) -> windows_sys::core::GUID {
        use windows_sys::Win32::UI::Shell as S;
        match self {
            KnownFolder::Desktop => S::FOLDERID_Desktop,
            KnownFolder::Downloads => S::FOLDERID_Downloads,
            KnownFolder::Documents => S::FOLDERID_Documents,
            KnownFolder::Pictures => S::FOLDERID_Pictures,
            KnownFolder::Music => S::FOLDERID_Music,
            KnownFolder::Videos => S::FOLDERID_Videos,
            KnownFolder::Profile => S::FOLDERID_Profile,
            KnownFolder::LocalAppData => S::FOLDERID_LocalAppData,
            KnownFolder::RoamingAppData => S::FOLDERID_RoamingAppData,
            KnownFolder::ProgramData => S::FOLDERID_ProgramData,
            KnownFolder::Windows => S::FOLDERID_Windows,
            KnownFolder::ProgramFiles => S::FOLDERID_ProgramFiles,
            KnownFolder::ProgramFilesX86 => S::FOLDERID_ProgramFilesX86,
            KnownFolder::Public => S::FOLDERID_Public,
            KnownFolder::RecycleBin => S::FOLDERID_RecycleBinFolder,
            KnownFolder::Computer => S::FOLDERID_ComputerFolder,
            KnownFolder::Network => S::FOLDERID_NetworkFolder,
        }
    }

    /// The on-disk path, or `None` for virtual shell folders.
    pub fn path(self) -> Option<std::path::PathBuf> {
        let guid = self.guid();
        let mut psz: *mut u16 = std::ptr::null_mut();
        let hr = unsafe { SHGetKnownFolderPath(&guid, 0, std::ptr::null_mut(), &mut psz) };
        if hr < 0 || psz.is_null() {
            return None;
        }
        let mut len = 0usize;
        while unsafe { *psz.add(len) } != 0 {
            len += 1;
        }
        let slice = unsafe { std::slice::from_raw_parts(psz, len) };
        let s = String::from_utf16_lossy(slice);
        unsafe {
            windows_sys::Win32::System::Com::CoTaskMemFree(psz as *const c_void);
        }
        if s.is_empty() { None } else { Some(std::path::PathBuf::from(s)) }
    }

    /// Everything Explorer lists in its sidebar tree.
    pub fn sidebar() -> &'static [KnownFolder] {
        &[
            KnownFolder::Profile,
            KnownFolder::Desktop,
            KnownFolder::Downloads,
            KnownFolder::Documents,
            KnownFolder::Pictures,
            KnownFolder::Music,
            KnownFolder::Videos,
            KnownFolder::Computer,
            KnownFolder::Network,
            KnownFolder::RecycleBin,
        ]
    }
}

/// Open a path (or URL) with its default handler, as a double-click would.
pub fn open(path: impl AsRef<Path>) -> Result<()> {
    let file = to_wide(path.as_ref());
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            str_to_wide("open").as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if rc as isize > 32 { Ok(()) } else { Err(WinError::Other(format!("ShellExecute failed ({} )", rc as isize))) }
}

/// Open with an explicit verb: "edit", "runas" (UAC elevation), "properties".
pub fn open_verb(path: impl AsRef<Path>, verb: &str) -> Result<()> {
    let file = to_wide(path.as_ref());
    let verb_w = str_to_wide(verb);
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if rc as isize > 32 { Ok(()) } else { Err(WinError::Other(format!("ShellExecute({verb}) failed ({})", rc as isize))) }
}

/// Show the shell's "Properties" dialog for a path.
pub fn show_properties(path: impl AsRef<Path>) -> Result<()> {
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC;
    info.nShow = SW_SHOWNORMAL;
    let verb = str_to_wide("properties");
    let file = to_wide(path.as_ref());
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    check("ShellExecuteExW", unsafe { ShellExecuteExW(&mut info) })
}

/// Launch an executable with command-line arguments, waiting for the handle.
///
/// Returns the process id when the shell reports one.
pub fn execute(path: impl AsRef<Path>, args: &str, working_dir: Option<&Path>) -> Result<Option<u32>> {
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    let file = to_wide(path.as_ref());
    let params = str_to_wide(args);
    let dir = working_dir.map(to_wide);
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    if let Some(d) = &dir {
        info.lpDirectory = d.as_ptr();
    }
    info.nShow = SW_SHOWNORMAL;
    check("ShellExecuteExW", unsafe { ShellExecuteExW(&mut info) })?;
    let pid = if info.hProcess.is_null() {
        None
    } else {
        let pid = unsafe { windows_sys::Win32::System::Threading::GetProcessId(info.hProcess) };
        // wrap in an owned handle so it closes when scope ends
        let _owned = Handle::new(info.hProcess);
        if pid == 0 { None } else { Some(pid) }
    };
    Ok(pid)
}

/// Move paths to the Recycle Bin (Explorer "Delete"), with shell undo support.
pub fn recycle(paths: &[std::path::PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    // SHFileOperation needs a double-NUL-terminated list.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.as_os_str().encode_wide_win());
        from.push(0);
    }
    from.push(0);

    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr() as *mut u16;
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_WANTNUKEWARNING) as u16;
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 {
        return Err(WinError::Other(format!("SHFileOperationW failed with {rc}")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(WinError::Other("delete was cancelled".into()));
    }
    Ok(())
}

use std::os::windows::ffi::OsStrExt;

trait EncodeWideWin {
    fn encode_wide_win(&self) -> Vec<u16>;
}
impl EncodeWideWin for std::ffi::OsStr {
    fn encode_wide_win(&self) -> Vec<u16> {
        self.encode_wide().collect()
    }
}

/// Small icon plus shell type name / display name for one path.
#[derive(Debug, Clone, Default)]
pub struct ShellItemInfo {
    pub display_name: String,
    pub type_name: String,
    /// RGBA pixels of a 16x16 icon, ready for an `egui` texture.
    pub icon_rgba: Option<Vec<u8>>,
}

/// Fetch icon + names for a path through `SHGetFileInfoW`.
///
/// The icon is converted to raw RGBA here (GDI → `GetDIBits`) so UI crates do
/// not need GDI code of their own.
pub fn file_info(path: impl AsRef<Path>, want_icon: bool) -> ShellItemInfo {
    let w = to_wide(path.as_ref());
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let flags = SHGFI_DISPLAYNAME | SHGFI_TYPENAME | if want_icon { SHGFI_ICON | SHGFI_SMALLICON } else { 0 };
    let ok = unsafe {
        SHGetFileInfoW(
            w.as_ptr(),
            if want_icon { 0x80 /* FILE_ATTRIBUTE_NORMAL */ } else { 0 },
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags | if want_icon { SHGFI_USEFILEATTRIBUTES } else { 0 },
        )
    };
    if ok == 0 {
        return ShellItemInfo::default();
    }
    let icon_rgba = if want_icon && !info.hIcon.is_null() {
        let rgba = unsafe { crate::icon::hicon_to_rgba(info.hIcon, 16, 16) };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon) };
        rgba
    } else {
        None
    };
    ShellItemInfo {
        display_name: from_wide(&info.szDisplayName, info.szDisplayName.len()),
        type_name: from_wide(&info.szTypeName, info.szTypeName.len()),
        icon_rgba,
    }
}

/// Resolve a shell item's display name only (cheap, no GDI).
pub fn display_name(path: impl AsRef<Path>) -> String {
    file_info(path, false).display_name
}

/// Open with elevated privileges (UAC prompt) — Explorer's "Run as administrator".
pub fn run_as_admin(path: impl AsRef<Path>) -> Result<()> {
    open_verb(path, "runas")
}

/// Launch a program through the current user's PATH (e.g. `notepad.exe`).
pub fn launch(command: &str, args: &str) -> Result<()> {
    let file = str_to_wide(command);
    let params = str_to_wide(args);
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            str_to_wide("open").as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if rc as isize > 32 { Ok(()) } else { Err(WinError::Other(format!("launch failed ({})", rc as isize))) }
}

/// The desktop window handle (used to parent modal dialogs).
pub fn desktop_hwnd() -> HWND {
    std::ptr::null_mut()
}

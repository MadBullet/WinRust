//! Shell integration: launching, known folders, icons, recycle bin.

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::wide::{from_wide, str_to_wide, to_wide};
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::path::Path;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Shell::{
    SHFileOperationW, SHGetFileInfoW, SHGetKnownFolderPath, ShellExecuteExW, ShellExecuteW,
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_WANTNUKEWARNING, FO_DELETE,
    SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHFILEINFOW, SHFILEOPSTRUCTW, SHGFI_DISPLAYNAME, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON,
    SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, SW_SHOWNORMAL};

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
            KnownFolder::Desktop => crate::i18n::ts("Desktop"),
            KnownFolder::Downloads => crate::i18n::ts("Downloads"),
            KnownFolder::Documents => crate::i18n::ts("Documents"),
            KnownFolder::Pictures => crate::i18n::ts("Pictures"),
            KnownFolder::Music => crate::i18n::ts("Music"),
            KnownFolder::Videos => crate::i18n::ts("Videos"),
            KnownFolder::Profile => crate::i18n::ts("Home"),
            KnownFolder::LocalAppData => crate::i18n::ts("Local AppData"),
            KnownFolder::RoamingAppData => crate::i18n::ts("Roaming AppData"),
            KnownFolder::ProgramData => crate::i18n::ts("ProgramData"),
            KnownFolder::Windows => crate::i18n::ts("Windows"),
            KnownFolder::ProgramFiles => crate::i18n::ts("Program Files"),
            KnownFolder::ProgramFilesX86 => crate::i18n::ts("Program Files (x86)"),
            KnownFolder::Public => crate::i18n::ts("Public"),
            KnownFolder::RecycleBin => crate::i18n::ts("Recycle Bin"),
            KnownFolder::Computer => crate::i18n::ts("This PC"),
            KnownFolder::Network => crate::i18n::ts("Network"),
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
        if s.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(s))
        }
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
    if rc as isize > 32 {
        Ok(())
    } else {
        Err(WinError::Other(format!(
            "ShellExecute failed ({} )",
            rc as isize
        )))
    }
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
    if rc as isize > 32 {
        Ok(())
    } else {
        Err(WinError::Other(format!(
            "ShellExecute({verb}) failed ({})",
            rc as isize
        )))
    }
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
pub fn execute(
    path: impl AsRef<Path>,
    args: &str,
    working_dir: Option<&Path>,
) -> Result<Option<u32>> {
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
        if pid == 0 {
            None
        } else {
            Some(pid)
        }
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
        return Err(WinError::Other(format!(
            "SHFileOperationW failed with {rc}"
        )));
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
///
/// `icon_size` selects the source icon: 16 for the details/list views and 32
/// for tiles/medium icons (Explorer uses 16/32/48/256).  0 disables the icon.
/// Unlike a first implementation this does **not** pass
/// `SHGFI_USEFILEATTRIBUTES`: the shell must look at the real file to pick
/// per-file icons (extracted exe icons, document thumbnails, folder art).
pub fn file_info_sized(path: impl AsRef<Path>, icon_size: u32) -> ShellItemInfo {
    let path = path.as_ref();
    let w = to_wide(path);
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let mut flags = SHGFI_DISPLAYNAME | SHGFI_TYPENAME;
    if icon_size > 0 {
        flags |= SHGFI_ICON;
        if icon_size <= 16 {
            flags |= SHGFI_SMALLICON;
        } else if icon_size <= 32 {
            flags |= SHGFI_LARGEICON;
        } else {
            // 48 px and above come from the system image list added in Vista.
            flags |= SHGFI_SYSICONINDEX;
        }
    }
    let ok = unsafe {
        SHGetFileInfoW(
            w.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ok == 0 {
        // Fall back to attribute-only lookup (works for paths that are not
        // currently reachable, e.g. a disconnected network drive).
        let attr = if path.is_dir() { 0x10 } else { 0x80 };
        let ok2 = unsafe {
            SHGetFileInfoW(
                w.as_ptr(),
                attr,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_DISPLAYNAME | SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
            )
        };
        if ok2 == 0 {
            return ShellItemInfo::default();
        }
        return ShellItemInfo {
            display_name: from_wide(&info.szDisplayName, info.szDisplayName.len()),
            type_name: from_wide(&info.szTypeName, info.szTypeName.len()),
            icon_rgba: None,
        };
    }
    let icon_rgba = if icon_size > 0 {
        if icon_size <= 32 {
            if info.hIcon.is_null() {
                None
            } else {
                let px = if icon_size <= 16 { 16 } else { 32 };
                let rgba = unsafe { crate::icon::hicon_to_rgba(info.hIcon, px, px) };
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon) };
                rgba
            }
        } else {
            // System image list: 0 = small, 1 = large, 2 = extra large (48),
            // 4 = jumbo (256).  We take 48 for tiles and 256 for thumbnails.
            crate::icon::system_icon_rgba(info.iIcon, icon_size)
        }
    } else {
        None
    };
    ShellItemInfo {
        display_name: from_wide(&info.szDisplayName, info.szDisplayName.len()),
        type_name: from_wide(&info.szTypeName, info.szTypeName.len()),
        icon_rgba,
    }
}

/// Backwards-compatible 16 px wrapper.
pub fn file_info(path: impl AsRef<Path>, want_icon: bool) -> ShellItemInfo {
    file_info_sized(path, if want_icon { 16 } else { 0 })
}

/// Explorer's localized "Type" text for a *file extension* — one shell call per
/// extension instead of one per file, so a 50 000-file folder does a handful.
pub fn type_name_for_extension(ext: &str, is_dir: bool) -> String {
    // `SHGetFileInfoW` on a fake `*.ext` name with USEFILEATTRIBUTES makes the
    // shell resolve the association without touching the disk.
    let probe = if is_dir {
        "folder".to_string()
    } else if ext.is_empty() {
        "file".to_string()
    } else {
        format!("probe.{ext}")
    };
    let w = to_wide(&probe);
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let attr = if is_dir { 0x10 } else { 0x80 };
    let ok = unsafe {
        SHGetFileInfoW(
            w.as_ptr(),
            attr,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if ok == 0 {
        return if is_dir {
            "File folder".into()
        } else {
            "File".into()
        };
    }
    from_wide(&info.szTypeName, info.szTypeName.len())
}

/// Read a text value from the clipboard (used for Ctrl+V of paths typed as text).
pub fn clipboard_text() -> Option<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT as u32);
        let mut out = None;
        if !handle.is_null() {
            let ptr = GlobalLock(handle) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    ptr, len,
                )));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        out
    }
}

/// Put text on the clipboard so it can be pasted into other applications.
pub fn set_clipboard_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            CloseClipboard();
            return false;
        }
        let dst = GlobalLock(mem) as *mut u16;
        if dst.is_null() {
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(mem);
        let ok = !SetClipboardData(CF_UNICODETEXT as u32, mem).is_null();
        CloseClipboard();
        ok
    }
}

/// Resolve a shell item's display name only (cheap, no GDI).
/// Version metadata embedded in an executable/DLL (`FileVersion`, `CompanyName`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionInfo {
    pub file_version: String,
    pub product_name: String,
    pub company: String,
    pub description: String,
}

/// Read the VS_VERSIONINFO block of a file.  Returns empty strings when the
/// file carries no version resource (plain data files, scripts, ...).
pub fn version_info(path: impl AsRef<Path>) -> VersionInfo {
    let wide = crate::wide::str_to_wide(&path.as_ref().to_string_lossy());
    let mut handle = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if size == 0 {
        return VersionInfo::default();
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr() as *mut _) } == 0 {
        return VersionInfo::default();
    }

    let mut out = VersionInfo::default();

    // Fixed block: gives us the numeric version.
    let mut fixed_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut fixed_len = 0u32;
    if unsafe {
        VerQueryValueW(
            buf.as_ptr() as *const _,
            w("\\").as_ptr(),
            &mut fixed_ptr,
            &mut fixed_len,
        )
    } != 0
        && fixed_len as usize >= std::mem::size_of::<VS_FIXEDFILEINFO>()
    {
        let ffi = unsafe { &*(fixed_ptr as *const VS_FIXEDFILEINFO) };
        let ms = (ffi.dwFileVersionMS >> 16, ffi.dwFileVersionMS & 0xFFFF);
        let ls = (ffi.dwFileVersionLS >> 16, ffi.dwFileVersionLS & 0xFFFF);
        out.file_version = format!("{}.{}.{}.{}", ms.0, ms.1, ls.0, ls.1);
    }

    // Strings live in `\StringFileInfo\<langcodepage>\<name>`.
    let mut trans_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut trans_len = 0u32;
    if unsafe {
        VerQueryValueW(
            buf.as_ptr() as *const _,
            w("\\VarFileInfo\\Translation").as_ptr(),
            &mut trans_ptr,
            &mut trans_len,
        )
    } != 0
        && trans_len >= 4
    {
        let words =
            unsafe { std::slice::from_raw_parts(trans_ptr as *const u16, trans_len as usize / 2) };
        let lang = words[0];
        let cp = words[1];
        for (key, slot) in [
            ("ProductName", &mut out.product_name),
            ("CompanyName", &mut out.company),
            ("FileDescription", &mut out.description),
        ] {
            let sub = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}");
            let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if unsafe {
                VerQueryValueW(buf.as_ptr() as *const _, w(&sub).as_ptr(), &mut p, &mut len)
            } != 0
                && len > 0
            {
                let chars = unsafe { std::slice::from_raw_parts(p as *const u16, len as usize) };
                *slot = crate::wide::from_wide(chars, chars.len())
                    .trim_end_matches('\0')
                    .to_string();
            }
        }
    }
    out
}

fn w(s: &str) -> Vec<u16> {
    crate::wide::str_to_wide(s)
}

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
    if rc as isize > 32 {
        Ok(())
    } else {
        Err(WinError::Other(format!("launch failed ({})", rc as isize)))
    }
}

/// The desktop window handle (used to parent modal dialogs).
pub fn desktop_hwnd() -> HWND {
    std::ptr::null_mut()
}

// ---------------------------------------------------------------------------
// Top-level windows (the desktop shell's taskbar)
// ---------------------------------------------------------------------------

/// One visible top-level window, as the taskbar shows it.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
    /// Image path of the owning process (empty when it cannot be opened).
    pub exe: String,
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len <= 0 {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let out = &mut *(lparam as *mut Vec<WindowInfo>);
    out.push(WindowInfo {
        hwnd: hwnd as isize,
        title: from_wide(&buf, len as usize),
        pid,
        exe: crate::process::exe_path_of(pid).unwrap_or_default(),
    });
    1
}

/// Run a registry "open" command on one file.
///
/// Registered commands are command lines, not argument lists: `"C:\Program
/// Files\App\app.exe" -open "%1"`.  This splits the executable off the front
/// (respecting the quotes Windows writes) and hands the rest, with `%1`
/// substituted, to `ShellExecuteW` - which is exactly how the shell treats
/// `HKCR\...\shell\open\command`.
pub fn run_command_for_file(command: &str, file: &Path) -> Result<()> {
    let command = command.trim();
    let (exe, rest) = if let Some(stripped) = command.strip_prefix('"') {
        match stripped.split_once('"') {
            Some((exe, rest)) => (exe.to_string(), rest.trim_start()),
            None => (stripped.to_string(), ""),
        }
    } else {
        match command.split_once(' ') {
            Some((exe, rest)) => (exe.to_string(), rest.trim_start()),
            None => (command.to_string(), ""),
        }
    };
    if exe.is_empty() {
        return Err(WinError::Other("empty command".into()));
    }
    // `%1` (and the unquoted `%L`) are the file; `%*` never appears in a
    // per-file handler.  A bare placeholder gets quotes so spaces survive.
    let target = file.display().to_string();
    let quoted = format!("\"{target}\"");
    let args = rest
        .replace("%1", &quoted)
        .replace("%l", &quoted)
        .replace("%L", &quoted);
    let args = if rest.is_empty() {
        quoted
    } else if args == rest {
        // The command has no placeholder: append the file.
        format!("{} {}", args, quoted).trim().to_string()
    } else {
        args
    };
    launch(&exe, args.trim())
}

/// Open a shell namespace entry by its `shell:` path (`shell:RecycleBinFolder`,
/// `shell:MyComputerFolder`).
///
/// Some entries have no useful filesystem path: the Recycle Bin's
/// `KnownFolderPath` is `C:\\$Recycle.Bin\\<SID>`, which Explorer cannot browse
/// as a normal folder, so it is opened through the namespace instead.  This is
/// exactly what the real desktop icon does.
pub fn open_shell_path(shell_path: &str) -> Result<()> {
    let verb = str_to_wide("open");
    let target = str_to_wide(shell_path);
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if rc as isize > 32 {
        Ok(())
    } else {
        Err(WinError::Other(format!(
            "ShellExecute({shell_path}) failed ({})",
            rc as isize
        )))
    }
}

/// The `shell:` path for the virtual folders, if the entry has one.
pub fn shell_path_of(folder: KnownFolder) -> Option<&'static str> {
    match folder {
        KnownFolder::RecycleBin => Some("shell:RecycleBinFolder"),
        KnownFolder::Computer => Some("shell:MyComputerFolder"),
        KnownFolder::Network => Some("shell:NetworkPlacesFolder"),
        _ => None,
    }
}

/// The process that owns the foreground window, if any.
pub fn foreground_pid() -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            None
        } else {
            Some(pid)
        }
    }
}

/// Is a left/right `Win` key held down right now?
///
/// egui has no `Key::Win`, and the shell is the only program that cares, so the
/// state is read straight from `GetAsyncKeyState`.  That also works when the
/// shell window itself is not focused - which is the point: `Win` must open
/// Start no matter which window the user was in.
pub fn win_key_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};
    const DOWN: i16 = 0x8000u16 as i16;
    unsafe {
        (GetAsyncKeyState(VK_LWIN as i32) & DOWN) != 0
            || (GetAsyncKeyState(VK_RWIN as i32) & DOWN) != 0
    }
}

/// Make the given window the foreground window and give it keyboard focus.
///
/// A shell at the bottom of the z-order never becomes foreground on its own, so
/// its egui context never sees `Win` or the tray hotkeys.  Clicking the bar has
/// to claim focus explicitly - `SetForegroundWindow` from the current foreground
/// process is allowed, and the shell is that process right after a click.
pub fn focus_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, SetForegroundWindow, SetFocus,
    };
    if hwnd == 0 {
        return;
    }
    let h = hwnd as HWND;
    unsafe {
        SetForegroundWindow(h);
        BringWindowToTop(h);
        SetFocus(h);
    }
}

/// Raise one specific window (a taskbar preview card was clicked).
pub fn activate_window_hwnd(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    if hwnd == 0 {
        return;
    }
    let h = hwnd as HWND;
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}

/// Every visible top-level window with a title, oldest first.
///
/// The callback allocates nothing that could unwind, which matters because a
/// panic crossing the FFI boundary is undefined behaviour.
pub fn top_level_windows() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        EnumWindows(
            Some(enum_window_proc),
            &mut out as *mut Vec<WindowInfo> as LPARAM,
        );
    }
    out
}

/// The icon a window actually shows in its title bar, as RGBA pixels.
///
/// The taskbar must show real application icons, not a placeholder: this asks
/// the window for `WM_GETICON` first (what its author installed), then falls
/// back to the small or big icon of the window class, and finally to the icon
/// of the executable itself.  `size` is 16, 32 or 48 px; the closest available
/// source is scaled by `draw_icon` on the way out.
pub fn window_icon_rgba(hwnd: isize, size: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassLongPtrW, GetIconInfo, SendMessageTimeoutW, GCLP_HICON, GCLP_HICONSM, ICONINFO,
        ICON_BIG, ICON_SMALL, ICON_SMALL2, SMTO_ABORTIFHUNG, WM_GETICON,
    };
    let h = hwnd as HWND;
    if h.is_null() {
        return None;
    }
    let px = if size <= 16 {
        16
    } else if size <= 32 {
        32
    } else {
        48
    };

    /// `WM_GETICON` answers with an `HICON` in the result word.
    unsafe fn query(h: HWND, which: u32) -> Option<*mut core::ffi::c_void> {
        let mut out: usize = 0;
        let ok = SendMessageTimeoutW(
            h,
            WM_GETICON,
            which as usize, // WPARAM
            0,              // LPARAM
            SMTO_ABORTIFHUNG,
            120,
            &mut out,
        );
        if ok != 0 && out != 0 {
            Some(out as *mut core::ffi::c_void)
        } else {
            None
        }
    }

    let mut hicon = None;
    unsafe {
        for which in [ICON_SMALL2, ICON_BIG, ICON_SMALL] {
            if let Some(ic) = query(h, which) {
                hicon = Some(ic);
                break;
            }
        }
        if hicon.is_none() {
            for idx in [GCLP_HICONSM, GCLP_HICON] {
                let ic = GetClassLongPtrW(h, idx) as *mut core::ffi::c_void;
                if !ic.is_null() {
                    hicon = Some(ic);
                    break;
                }
            }
        }
    }
    let hicon = hicon?;
    // `GetIconInfo` merely proves the handle is a bitmap icon before GDI work;
    // a private copy is not needed because we only read it.
    unsafe {
        let mut info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut info) == 0 {
            return None;
        }
        if !info.hbmColor.is_null() {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(info.hbmColor as _);
        }
        if !info.hbmMask.is_null() {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(info.hbmMask as _);
        }
        crate::icon::hicon_to_rgba(hicon, px, px)
    }
}

/// RGBA icon of an executable file at `size` px, for taskbar entries whose
/// window has no icon of its own.
pub fn exe_icon_rgba(exe: &str, size: u32) -> Option<Vec<u8>> {
    if exe.is_empty() {
        return None;
    }
    let px = if size <= 16 {
        16
    } else if size <= 32 {
        32
    } else {
        48
    };
    let info = file_info_sized(exe, px);
    info.icon_rgba
}

/// Bring a window to the foreground (Start-menu/taskbar click).
///
/// `SetForegroundWindow` is refused when the caller is not the foreground
/// process; the shell is, so the normal path works, and the fallback restores
/// a minimised window anyway.
pub fn activate_window(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    for w in top_level_windows() {
        if w.pid == pid {
            let hwnd = w.hwnd as HWND;
            unsafe {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
            }
        }
    }
}

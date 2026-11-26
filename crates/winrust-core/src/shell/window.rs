//! Top-level windows and the shell's own window: enumeration, icons, focus.
//!
//! Egui has no `Key::Win`, and the shell must open Start no matter which
//! window the user was in, so this module reads the keyboard and the window
//! list straight from `user32`.  It is deliberately separate from [`shell`]:
//! the window side needs the process image and the shell's file icons to draw
//! a taskbar button, while `shell` (launching, known folders, the recycle bin)
//! must not depend on `process`.  The edge runs `window` -> `shell`, one way.

use super::shell::file_info_sized;
use crate::wide::from_wide;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

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
        exe: crate::system::process::exe_path_of(pid).unwrap_or_default(),
    });
    1
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

/// Is the `V` key held down right now?  (`Win+V` opens the clipboard history.)
///
/// Same reasoning as [`win_key_down`]: the shell must see the combination
/// while another program has the text focus, and `GetAsyncKeyState` is the
/// only way to ask that from a background window.
pub fn v_key_down() -> bool {
    key_down(0x56)
}

/// Is the `I` key held down right now?  (`Win+I` opens Settings.)
pub fn i_key_down() -> bool {
    key_down(0x49)
}

/// Is the virtual key `vk` held down right now?
///
/// The one place `GetAsyncKeyState` is read.  `Win+V`, `Win+I` and any future
/// shell-wide chord all need the same thing - a key that egui cannot see,
/// from a window that is not focused - so the state is read here and the
/// callers only name the key.
pub fn key_down(vk: i32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    const DOWN: i16 = 0x8000u16 as i16;
    unsafe { (GetAsyncKeyState(vk) & DOWN) != 0 }
}

/// Make the given window the foreground window and give it keyboard focus.
///
/// A shell at the bottom of the z-order never becomes foreground on its own, so
/// its egui context never sees `Win` or the tray hotkeys.  Clicking the bar has
/// to claim focus explicitly - `SetForegroundWindow` from the current foreground
/// process is allowed, and the shell is that process right after a click.
pub fn focus_window(hwnd: isize) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
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

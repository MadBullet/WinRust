//! HICON → RGBA conversion.
//!
//! `egui` needs raw RGBA pixels; the shell hands us `HICON`s.  This module
//! renders the icon into a 32-bit top-down DIB with GDI and reads the pixels
//! back, handling the mask for legacy icons that have no alpha channel.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateCompatibleBitmap, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ,
};
use std::ffi::c_void;
use windows_sys::Win32::UI::WindowsAndMessaging::{DrawIconEx, GetIconInfo, HICON, ICONINFO, DI_MASK, DI_NORMAL};

/// Convert an icon into tightly packed RGBA8 pixels (row-major, top-down).
///
/// Returns `None` if GDI refuses to render the icon (extremely rare).
///
/// # Safety
/// `hicon` must be a valid icon handle that stays alive for the call.
pub unsafe fn hicon_to_rgba(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON, w: u32, h: u32) -> Option<Vec<u8>> {
    if hicon.is_null() || w == 0 || h == 0 {
        return None;
    }
    unsafe {
        let screen: HDC = GetDC(std::ptr::null_mut() as HWND);
        if screen.is_null() {
            return None;
        }
        let mem: HDC = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut() as HWND, screen);
            return None;
        }

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: w * h * 4,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib: HBITMAP = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut() as HWND, screen);
            return None;
        }

        let old: HGDIOBJ = SelectObject(mem, dib as HGDIOBJ);

        // Fill transparent black first.
        std::ptr::write_bytes(bits as *mut u8, 0, (w * h * 4) as usize);

        // Draw the icon (DI_NORMAL blends color+mask with alpha).
        let drawn = DrawIconEx(mem, 0, 0, hicon, w as i32, h as i32, 0, std::ptr::null_mut(), DI_NORMAL);
        if drawn == 0 {
            // Fallback: legacy icons drawn via mask only.
            DrawIconEx(mem, 0, 0, hicon, w as i32, h as i32, 0, std::ptr::null_mut(), DI_MASK);
        }

        let mut rgba = vec![0u8; (w * h * 4) as usize];
        std::ptr::copy_nonoverlapping(bits as *const u8, rgba.as_mut_ptr(), rgba.len());

        // GDI writes BGRA; swap to RGBA.  Also force fully-opaque icons fully visible.
        let mut any_alpha = false;
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            if px[3] != 0 {
                any_alpha = true;
            }
        }
        if !any_alpha {
            for px in rgba.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }

        // Legacy icons without an alpha channel: derive alpha from the mask.
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut ii) != 0 && !ii.hbmMask.is_null() {
            if ii.hbmColor.is_null() {
                // Pure mask icon — render the mask as a black shape.
                for px in rgba.chunks_exact_mut(4) {
                    let a = px[3];
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                    px[3] = if a == 0 { 255 } else { 0 };
                }
            }
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask as HGDIOBJ);
            }
            if !ii.hbmColor.is_null() {
                DeleteObject(ii.hbmColor as HGDIOBJ);
            }
        }

        SelectObject(mem, old);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut() as HWND, screen);

        Some(rgba)
    }
}

/// Fallback: a solid placeholder tile (used when no icon can be obtained).
pub fn placeholder_rgba(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        v.extend_from_slice(&rgba);
    }
    v
}

/// Unused GDI import retained so the linker keeps `CreateCompatibleBitmap`
/// available for the legacy path in older Windows builds.
#[allow(dead_code)]
fn _reserve(dc: HDC) -> HBITMAP {
    unsafe { CreateCompatibleBitmap(dc, 1, 1) }
}


// ---------------------------------------------------------------------------
// System image list (large icons: 32 / 48 / 256 px)
// ---------------------------------------------------------------------------

/// RAII COM apartment for the calling thread.
///
/// The shell image-list APIs are COM objects, so they need an initialized
/// apartment.  `eframe` does not initialize COM for us, and calling
/// `CoInitializeEx` twice on one thread is legal (it returns `S_FALSE`), so the
/// guard is cheap to create around each call.
pub struct ComGuard {
    should_uninit: bool,
}

impl ComGuard {
    pub fn new() -> Self {
        let hr = unsafe {
            windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32,
            )
        };
        // S_OK (0) and S_FALSE (1) both mean "usable"; RPC_E_CHANGED_MODE
        // (0x80010106) means another apartment already exists — also usable.
        Self { should_uninit: hr == 0 || hr == 1 }
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// Minimal `IImageList` vtable — only the slots we call.
///
/// `IImageList` has no binding in `windows-sys` 0.59, but its ABI is stable and
/// documented.  The public vtable is `IUnknown` (3 slots) followed by:
/// `Add`, `ReplaceIcon`, `SetOverlayImage`, `Replace`, `AddMasked`, `Draw`,
/// `Remove`, `GetIcon`.  So `GetIcon` is slot 3+8 = 11.
#[repr(C)]
struct IImageListVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const windows_sys::core::GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // Add(HBITMAP hbmImage, HBITMAP hbmMask, int *pi)
    add: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *mut i32) -> i32,
    // ReplaceIcon(int i, HICON hicon, int *pi)
    replace_icon: unsafe extern "system" fn(*mut c_void, i32, HICON, *mut i32) -> i32,
    // SetOverlayImage(int iImage, int iOverlay)
    set_overlay_image: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32,
    // Replace(int i, HBITMAP hbmImage, HBITMAP hbmMask)
    replace: unsafe extern "system" fn(*mut c_void, i32, *mut c_void, *mut c_void) -> i32,
    // AddMasked(HBITMAP hbmImage, COLORREF crMask, int *pi)
    add_masked: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut i32) -> i32,
    // Draw(IMAGELISTDRAWPARAMS *pimldp)
    draw: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    // Remove(int i)
    remove: unsafe extern "system" fn(*mut c_void, i32) -> i32,
    // GetIcon(int i, uint flags, HICON *picon)
    get_icon: unsafe extern "system" fn(*mut c_void, i32, u32, *mut HICON) -> i32,
}

/// `IID_IImageList` — `{46EB5926-582E-4017-9FDF-E8998DAA0950}`.
const IID_IIMAGE_LIST: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x46EB5926,
    data2: 0x582E,
    data3: 0x4017,
    data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50],
};

/// Render icon `index` from the system image list at `size` px (32, 48 or 256).
pub fn system_icon_rgba(index: i32, size: u32) -> Option<Vec<u8>> {
    if index < 0 {
        return None;
    }
    let _com = ComGuard::new();
    // SHIL_LARGE = 0 (32 px), SHIL_EXTRALARGE = 2 (48 px), SHIL_JUMBO = 4 (256 px)
    let shil = match size {
        0..=32 => 0u32,
        33..=48 => 2u32,
        _ => 4u32,
    };
    let mut list: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        windows_sys::Win32::UI::Shell::SHGetImageList(
            shil as i32,
            &IID_IIMAGE_LIST,
            &mut list,
        )
    };
    if hr < 0 || list.is_null() {
        return None;
    }
    let vtbl = unsafe { *(list as *mut *const IImageListVtbl) };
    if vtbl.is_null() {
        return None;
    }
    let mut hicon: HICON = std::ptr::null_mut();
    let get_icon = unsafe { (*vtbl).get_icon };
    let rc = unsafe { get_icon(list, index, 0x00000001 /* ILD_TRANSPARENT */, &mut hicon) };
    if rc < 0 || hicon.is_null() {
        return None;
    }
    let px = match size {
        0..=32 => 32,
        33..=48 => 48,
        _ => 256,
    };
    let rgba = unsafe { hicon_to_rgba(hicon, px, px) };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(hicon);
        let release = (*vtbl).release;
        release(list);
    }
    rgba
}

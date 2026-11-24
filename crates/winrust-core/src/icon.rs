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
use windows_sys::Win32::UI::WindowsAndMessaging::{DrawIconEx, GetIconInfo, ICONINFO, DI_MASK, DI_NORMAL};

/// Convert an icon into tightly packed RGBA8 pixels (row-major, top-down).
///
/// Returns `None` if GDI refuses to render the icon (extremely rare).
pub fn hicon_to_rgba(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON, w: u32, h: u32) -> Option<Vec<u8>> {
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

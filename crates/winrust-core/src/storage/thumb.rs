//! Window thumbnails for the taskbar's hover previews.
//!
//! The real taskbar shows a live miniature of a window when the pointer rests
//! on its button.  The mechanism here is the one screenshot tools use:
//! `PrintWindow(PW_RENDERFULLCONTENT)` into a memory DC, which also captures
//! windows that are covered by other windows (the flag is what makes
//! hardware-accelerated and layered surfaces show up).
//!
//! The bitmap is scaled with `StretchBlt(HALFTONE)` and returned as top-down
//! RGBA, ready for `egui::ColorImage`.  Everything is best-effort: a protected
//! window returns `None` and the preview falls back to the window's title.

/// A captured window picture: RGBA, top-down, `width * height * 4` bytes.
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Capture `hwnd` scaled into a `max_w` x `max_h` box, keeping its shape.
///
/// Returns `None` when the window is gone, has no area, or refuses to render.
pub fn window_thumbnail(hwnd: isize, max_w: u32, max_h: u32) -> Option<Thumbnail> {
    if hwnd == 0 || max_w == 0 || max_h == 0 {
        return None;
    }
    let (w, h) = window_size(hwnd)?;
    let scale = (max_w as f32 / w as f32)
        .min(max_h as f32 / h as f32)
        .min(1.0);
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);

    unsafe {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, SRCCOPY,
        };
        // `PrintWindow` lives under `Storage::Xps` in windows-sys (a historical
        // quirk of the crate's metadata), not under `WindowsAndMessaging`.
        use windows_sys::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};

        let window = hwnd as *mut core::ffi::c_void;
        // The window's own DC gives us a compatible colour depth.
        let window_dc = GetDC(window);
        if window_dc.is_null() {
            return None;
        }
        let result = (|| {
            let src_dc = CreateCompatibleDC(window_dc);
            if src_dc.is_null() {
                return None;
            }
            let src_bmp = CreateCompatibleBitmap(window_dc, w as i32, h as i32);
            if src_bmp.is_null() {
                DeleteDC(src_dc);
                return None;
            }
            let old_src = SelectObject(src_dc, src_bmp);
            // 2 = PW_RENDERFULLCONTENT.
            let rendered = PrintWindow(window, src_dc, 2 as PRINT_WINDOW_FLAGS);
            let mut out = None;
            if rendered != 0 {
                // A second DC holds the scaled copy; StretchBlt does the
                // resampling so the shrunken picture is not aliased noise.
                let dst_dc = CreateCompatibleDC(window_dc);
                if !dst_dc.is_null() {
                    let dst_bmp = CreateCompatibleBitmap(window_dc, tw as i32, th as i32);
                    if !dst_bmp.is_null() {
                        let old_dst = SelectObject(dst_dc, dst_bmp);
                        if windows_sys::Win32::Graphics::Gdi::StretchBlt(
                            dst_dc, 0, 0, tw as i32, th as i32, src_dc, 0, 0, w as i32, h as i32,
                            SRCCOPY,
                        ) != 0
                        {
                            out = read_rgba(dst_dc, dst_bmp, tw, th);
                        }
                        SelectObject(dst_dc, old_dst);
                        DeleteObject(dst_bmp);
                    }
                    DeleteDC(dst_dc);
                }
            }
            SelectObject(src_dc, old_src);
            DeleteObject(src_bmp);
            DeleteDC(src_dc);
            out
        })();
        ReleaseDC(window, window_dc);
        result
    }
}

/// Bits of a selected bitmap as RGBA, top-down.
unsafe fn read_rgba(
    dc: *mut core::ffi::c_void,
    bitmap: *mut core::ffi::c_void,
    w: u32,
    h: u32,
) -> Option<Thumbnail> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    let mut header: BITMAPINFOHEADER = std::mem::zeroed();
    header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    header.biWidth = w as i32;
    // A negative height asks GDI for top-down rows, which is the order
    // egui's `ColorImage` expects.
    header.biHeight = -(h as i32);
    header.biPlanes = 1;
    header.biBitCount = 32;
    header.biCompression = BI_RGB;
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader = header;

    let mut buf = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        h,
        buf.as_mut_ptr() as *mut core::ffi::c_void,
        &mut info,
        DIB_RGB_COLORS,
    );
    if lines == 0 {
        return None;
    }
    // GDI hands out BGRA with a zero (or garbage) alpha channel: PrintWindow
    // does not fill alpha, so every pixel would be transparent.  Force it
    // opaque and swap the channels.
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Some(Thumbnail {
        width: w,
        height: h,
        rgba: buf,
    })
}

/// Pixel size of a window, or `None` when it has no area.
pub fn window_size(hwnd: isize) -> Option<(u32, u32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd as *mut core::ffi::c_void, &mut r) == 0 {
            return None;
        }
        let w = (r.right - r.left).max(0) as u32;
        let h = (r.bottom - r.top).max(0) as u32;
        (w > 1 && h > 1).then_some((w, h))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_null_window_has_no_thumbnail() {
        assert!(crate::storage::thumb::window_thumbnail(0, 320, 200).is_none());
        assert!(crate::storage::thumb::window_size(0).is_none());
    }
}

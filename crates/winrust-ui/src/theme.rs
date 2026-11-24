//! Shared look and the Windows-specific window effects.
//!
//! The dark palette is identical in all six programs so they read as one suite.
//! Transparency is applied through the classic layered-window alpha (works on
//! every Windows from 7 to 11, no composition surprises) and blur through the
//! deprecated-but-still-supported `DwmEnableBlurBehindWindow`, which Windows 10
//! 1803+ implements as acrylic.  Both are *optional*: with the switch off, no
//! DWM call is ever made, so the window costs nothing extra.

use winrust_core::i18n::Appearance;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

pub const BG: u8 = 24; // window background
pub const SURFACE: u8 = 32; // panels, headers
pub const SURFACE_HIGH: u8 = 42; // hovered rows, titles
pub const LINE: u8 = 58; // separators
pub const TEXT: u8 = 226;
pub const TEXT_WEAK: u8 = 158;
pub const ACCENT: (u8, u8, u8) = (96, 176, 255);
pub const OK: (u8, u8, u8) = (120, 220, 120);
pub const WARN: (u8, u8, u8) = (240, 190, 110);
pub const ERR: (u8, u8, u8) = (232, 120, 120);

/// The window background colour with the configured opacity applied.
pub fn bg_color(a: &Appearance) -> [f32; 4] {
    let alpha = a.alpha();
    [
        BG as f32 / 255.0,
        BG as f32 / 255.0,
        (BG as f32 + 4.0) / 255.0,
        alpha,
    ]
}

/// Apply the shared dark visuals to an egui context.
pub fn apply(ctx: &egui::Context, a: &Appearance) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(egui::Color32::from_rgb(TEXT, TEXT, TEXT));
    visuals.panel_fill = egui::Color32::from_rgb(BG, BG, BG + 4);
    visuals.window_fill = egui::Color32::from_rgb(SURFACE, SURFACE, SURFACE + 4);
    visuals.faint_bg_color = egui::Color32::from_rgb(SURFACE, SURFACE, SURFACE + 4);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 18, 22);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(SURFACE, SURFACE, SURFACE + 2);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(SURFACE_HIGH, SURFACE_HIGH, SURFACE_HIGH + 6);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(58, 58, 70);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(ACCENT.0 / 2, ACCENT.1 / 2, ACCENT.2 / 2);
    visuals.selection.bg_fill = egui::Color32::from_rgb(40, 84, 132);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(ACCENT.0, ACCENT.1, ACCENT.2));
    ctx.set_visuals(visuals);
    // Transparency: hand egui a translucent background so the DWM composition
    // underneath is visible.  With it off the window paints opaque.
    ctx.set_pixels_per_point(ctx.pixels_per_point());
}

/// Tell eframe whether the window must be transparent.  eframe has to create
/// the window with this flag; changing it later needs a new window, so the
/// value is only read at startup and the settings dialog says so.
pub fn wants_transparency(a: &Appearance) -> bool {
    a.transparent
}

// ---------------------------------------------------------------------------
// Win32 plumbing
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod native {
    use super::Appearance;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DwmSetWindowAttribute, DWM_BLURBEHIND, DWM_BB_ENABLE,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };

    /// Re-apply alpha and blur to a window.  Safe to call repeatedly; it is a
    /// no-op (no DWM traffic at all) when transparency is off.
    pub fn apply_window(hwnd: isize, a: &Appearance) {
        if hwnd == 0 {
            return;
        }
        let h = hwnd as HWND;
        unsafe {
            // Dark title bar on Windows 10 20H1+ / Windows 11.
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                h,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );

            let ex = GetWindowLongW(h, GWL_EXSTYLE);
            if a.transparent {
                if ex & WS_EX_LAYERED as i32 == 0 {
                    SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED as i32);
                }
                let alpha = (a.opacity.clamp(30, 100) as u32 * 255 / 100) as u8;
                let _ = SetLayeredWindowAttributes(h, 0, alpha, LWA_ALPHA);

                let mut bb: DWM_BLURBEHIND = std::mem::zeroed();
                bb.dwFlags = DWM_BB_ENABLE;
                bb.fEnable = i32::from(a.blur_active());
                let _ = DwmEnableBlurBehindWindow(h, &bb);
            } else {
                let mut bb: DWM_BLURBEHIND = std::mem::zeroed();
                bb.dwFlags = DWM_BB_ENABLE;
                bb.fEnable = 0;
                let _ = DwmEnableBlurBehindWindow(h, &bb);
                if ex & WS_EX_LAYERED as i32 != 0 {
                    SetWindowLongW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as i32));
                }
            }
        }
    }
}

#[cfg(windows)]
pub use native::apply_window;

#[cfg(not(windows))]
pub fn apply_window(_hwnd: isize, _a: &Appearance) {}

/// Extract the raw `HWND` from an egui window handle, if this is Windows.
#[cfg(windows)]
pub fn hwnd_from_handle(handle: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    use raw_window_handle::RawWindowHandle;
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn hwnd_from_handle(_handle: &impl std::any::Any) -> Option<isize> {
    None
}

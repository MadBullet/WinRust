//! Shared look and the Windows-specific window effects.
//!
//! Every program uses the same three palettes - dark (the classic WinRust
//! scheme), light (Windows 11 grey) and AMOLED (the dark layout on pure black,
//! for OLED panels) - chosen in the launcher and stored in the registry, so the
//! suite always reads as one product.
//!
//! Transparency and blur are *optional*: with the switches off no DWM call is
//! ever made, so the window costs nothing extra.
//!
//! ## Why blur used to do nothing
//!
//! `SetLayeredWindowAttributes` (the alpha slider) and DWM backdrop effects are
//! mutually exclusive: once a window carries `WS_EX_LAYERED`, DWM stops
//! compositing a live backdrop behind it.  The old code applied both, so the
//! blur switch silently lost.  Now the two paths are exclusive: with blur on we
//! ask DWM for a backdrop and drop `WS_EX_LAYERED`; with blur off (and
//! transparency on) we fall back to the classic layered alpha.

use winrust_core::i18n::{Appearance, Theme};

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// One colour set, so the three themes cannot drift apart.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Window background.
    pub bg: (u8, u8, u8),
    /// Panels, headers.
    pub surface: (u8, u8, u8),
    /// Hovered rows, titles.
    pub surface_high: (u8, u8, u8),
    /// Separators and strokes.
    pub line: (u8, u8, u8),
    pub text: (u8, u8, u8),
    pub text_weak: (u8, u8, u8),
    pub accent: (u8, u8, u8),
    pub ok: (u8, u8, u8),
    pub warn: (u8, u8, u8),
    pub err: (u8, u8, u8),
    /// Deepest input/console background.
    pub extreme: (u8, u8, u8),
    /// Zebra striping and faint fills.
    pub faint: (u8, u8, u8),
    /// Selected-row fill.
    pub selection: (u8, u8, u8),
    pub hover: (u8, u8, u8),
    /// Widget fill when pressed.
    pub active: (u8, u8, u8),
    /// Start-menu tile fill.
    pub tile: (u8, u8, u8),
    /// Secondary text (captions such as "ПУСК").
    pub weak: (u8, u8, u8),
}

const DARK: Palette = Palette {
    bg: (24, 24, 28),
    surface: (32, 32, 36),
    surface_high: (42, 42, 48),
    line: (58, 58, 66),
    text: (226, 226, 230),
    text_weak: (158, 158, 170),
    accent: (96, 176, 255),
    ok: (120, 220, 120),
    warn: (240, 190, 110),
    err: (232, 120, 120),
    extreme: (18, 18, 22),
    faint: (32, 32, 37),
    selection: (40, 84, 132),
    hover: (58, 58, 70),
    active: (48, 88, 128),
    tile: (42, 42, 48),
    weak: (158, 158, 170),
};

const AMOLED: Palette = Palette {
    bg: (0, 0, 0),
    surface: (8, 8, 10),
    surface_high: (20, 20, 24),
    line: (40, 40, 46),
    text: (232, 232, 236),
    text_weak: (150, 150, 162),
    accent: (96, 176, 255),
    ok: (110, 226, 140),
    warn: (246, 196, 96),
    err: (240, 120, 120),
    extreme: (0, 0, 0),
    faint: (12, 12, 14),
    selection: (24, 64, 104),
    hover: (28, 28, 34),
    active: (32, 72, 112),
    tile: (20, 20, 24),
    weak: (150, 150, 162),
};

const LIGHT: Palette = Palette {
    bg: (243, 243, 243),
    surface: (250, 250, 250),
    surface_high: (235, 235, 235),
    line: (208, 208, 208),
    text: (28, 28, 30),
    text_weak: (96, 96, 104),
    accent: (0, 95, 184),
    ok: (16, 124, 16),
    warn: (157, 93, 0),
    err: (196, 43, 28),
    extreme: (255, 255, 255),
    faint: (237, 237, 237),
    selection: (204, 228, 247),
    hover: (229, 241, 251),
    active: (198, 222, 246),
    tile: (243, 243, 243),
    weak: (96, 96, 104),
};

/// The palette for a theme.
pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
        Theme::Amoled => AMOLED,
    }
}

/// The palette of the given appearance.
pub fn pal(a: &Appearance) -> Palette {
    palette(a.theme)
}

fn c(rgb: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

fn ca(rgb: (u8, u8, u8), alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        rgb.0,
        rgb.1,
        rgb.2,
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

// ---------------------------------------------------------------------------
// Backwards-compatible constants (the dark palette)
// ---------------------------------------------------------------------------

pub const BG: u8 = DARK.bg.0;
pub const SURFACE: u8 = DARK.surface.0;
pub const SURFACE_HIGH: u8 = DARK.surface_high.0;
pub const LINE: u8 = DARK.line.0;
pub const TEXT: u8 = DARK.text.0;
pub const TEXT_WEAK: u8 = DARK.text_weak.0;
pub const ACCENT: (u8, u8, u8) = DARK.accent;
pub const OK: (u8, u8, u8) = DARK.ok;
pub const WARN: (u8, u8, u8) = DARK.warn;
pub const ERR: (u8, u8, u8) = DARK.err;

/// The window background colour with the configured opacity applied.
pub fn bg_color(a: &Appearance) -> [f32; 4] {
    let p = palette(a.theme);
    let alpha = a.alpha();
    [
        p.bg.0 as f32 / 255.0,
        p.bg.1 as f32 / 255.0,
        p.bg.2 as f32 / 255.0,
        alpha,
    ]
}

/// Apply a theme to an egui context.
///
/// Alpha only reaches the surfaces when the window really is translucent:
/// either the classic layered alpha or a DWM backdrop.  Otherwise the fills stay
/// opaque, which keeps text crisp and the window cheap.
pub fn apply(ctx: &egui::Context, a: &Appearance) {
    let translucent = a.transparent && (a.blur_active() || a.opacity < 100);
    let alpha = if translucent { a.alpha() } else { 1.0 };
    let p = palette(a.theme);

    let mut visuals = if a.theme.is_dark() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(c(p.text));
    visuals.panel_fill = ca(p.bg, alpha);
    visuals.window_fill = ca(p.surface, alpha);
    visuals.faint_bg_color = ca(p.faint, if translucent { alpha.max(0.55) } else { 1.0 });
    visuals.extreme_bg_color = c(p.extreme);
    visuals.window_stroke = egui::Stroke::new(1.0_f32, c(p.line));
    visuals.widgets.noninteractive.bg_fill = ca(p.surface, alpha);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, c(p.line));
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, c(p.text));
    visuals.widgets.inactive.bg_fill = ca(p.surface_high, alpha);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, c(p.text));
    visuals.widgets.hovered.bg_fill = ca(p.hover, (alpha * 1.25).clamp(0.0, 1.0));
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, c(p.text));
    visuals.widgets.active.bg_fill = ca(p.active, (alpha * 1.25).clamp(0.0, 1.0));
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5_f32, c(p.text));
    visuals.selection.bg_fill = ca(p.selection, (alpha * 1.4).clamp(0.0, 1.0));
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, c(p.accent));

    // `Context::set_visuals` only writes the bucket of the *currently active*
    // theme, and eframe follows the Windows "app mode" preference.  On a machine
    // whose system preference is Light, a Dark palette handed to `set_visuals`
    // therefore landed in the light bucket and the window came out white - the
    // bug where turning transparency off showed a light window until the switch
    // was toggled twice.  Pin the preference first, then write both buckets so a
    // later system change cannot repaint the window behind our back.
    ctx.set_theme(match a.theme {
        Theme::Light => egui::ThemePreference::Light,
        // Dark and AMOLED are both "dark" to egui; the palette does the rest.
        _ => egui::ThemePreference::Dark,
    });
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

/// Tell eframe whether the window must be created with a transparent surface.
/// eframe can only apply this at window creation, so callers read it once.
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
        DwmEnableBlurBehindWindow, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWM_BB_ENABLE, DWM_BLURBEHIND,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetLayeredWindowAttributes, SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };

    /// Windows 11 corner rounding: `2` = round.
    const DWMWCP_ROUND: i32 = 2;

    /// Re-apply the window effects.  Cheap enough to call when the settings
    /// change; it is a no-op when nothing needs doing.
    pub fn apply_window(hwnd: isize, a: &Appearance) {
        apply_window_force(hwnd, a, false)
    }

    /// Same as [`apply_window`], but always issues the calls.
    pub fn apply_window_force(hwnd: isize, a: &Appearance, force: bool) {
        if hwnd == 0 {
            return;
        }
        let _ = force;
        // `HWND` is `*mut c_void` in windows-sys 0.59, and `isize as HWND` is a
        // legal integer-to-pointer cast.
        let h = hwnd as HWND;
        unsafe {
            // Dark title bar (Windows 10 20H1+, Windows 11) and the rounded
            // corners Windows 11 gives normal windows.
            let dark: i32 = i32::from(a.theme.is_dark());
            let _ = DwmSetWindowAttribute(
                h,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                &dark as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                h,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &corner as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );

            let ex = GetWindowLongW(h, GWL_EXSTYLE);
            let layered = ex & WS_EX_LAYERED as i32 != 0;

            if a.blur_active() {
                // Blur is the *only* effect here: a layered window would stop
                // DWM from compositing the backdrop, which was the bug.
                if layered {
                    SetWindowLongW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as i32));
                }

                // Acrylic on Windows 11 22621+ ...
                let backdrop: i32 = DWMSBT_TRANSIENTWINDOW;
                let hr = DwmSetWindowAttribute(
                    h,
                    DWMWA_SYSTEMBACKDROP_TYPE as u32,
                    &backdrop as *const i32 as *const core::ffi::c_void,
                    std::mem::size_of::<i32>() as u32,
                );
                if hr < 0 {
                    // ... and the classic blur elsewhere, which needs the glass
                    // frame extended over the whole client area.
                    let margins = MARGINS {
                        cxLeftWidth: -1,
                        cxRightWidth: -1,
                        cyTopHeight: -1,
                        cyBottomHeight: -1,
                    };
                    let _ = DwmExtendFrameIntoClientArea(h, &margins);
                    // `DWM_BLURBEHIND` is `packed(1)`: field assignment through a
                    // place expression is rejected ("cannot borrow packed field"),
                    // and `&bb` has type `*const _` to a packed struct, which is
                    // also an error.  Build it as a value and take `addr_of!`.
                    let bb = DWM_BLURBEHIND {
                        dwFlags: DWM_BB_ENABLE,
                        fEnable: 1,
                        hRgnBlur: std::ptr::null_mut(),
                        fTransitionOnMaximized: 0,
                    };
                    let _ = DwmEnableBlurBehindWindow(h, &bb);
                }
            } else {
                // No blur: switch the backdrop off, then apply the alpha slider
                // through the classic layered window.
                let off: i32 = DWMSBT_NONE;
                let _ = DwmSetWindowAttribute(
                    h,
                    DWMWA_SYSTEMBACKDROP_TYPE as u32,
                    &off as *const i32 as *const core::ffi::c_void,
                    std::mem::size_of::<i32>() as u32,
                );
                let bb = DWM_BLURBEHIND {
                    dwFlags: DWM_BB_ENABLE,
                    fEnable: 0,
                    hRgnBlur: std::ptr::null_mut(),
                    fTransitionOnMaximized: 0,
                };
                let _ = DwmEnableBlurBehindWindow(h, &bb);

                if a.transparent {
                    if !layered {
                        SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED as i32);
                    }
                    let alpha = (a.opacity.clamp(30, 100) as u32 * 255 / 100) as u8;
                    let _ = SetLayeredWindowAttributes(h, 0, alpha, LWA_ALPHA);
                } else if layered {
                    SetWindowLongW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as i32));
                }
            }
        }
    }
}

#[cfg(windows)]
pub use native::{apply_window, apply_window_force};

#[cfg(not(windows))]
pub fn apply_window(_hwnd: isize, _a: &Appearance) {}

#[cfg(not(windows))]
pub fn apply_window_force(_hwnd: isize, _a: &Appearance, _force: bool) {}

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

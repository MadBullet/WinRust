//! The taskbar strip: Start, running programs, and the Windows-style tray.
//!
//! The layout follows Windows 10/11 rather than a generic toolbar:
//!
//! * **left** - the Start button, and nothing else.  Right-clicking it opens the
//!   Win+X menu (Task Manager, Settings, Explorer, ...).
//! * **centre-left** - one button per *running* program.  Pinned-but-closed
//!   tools are not shown, because the request was explicit: only active
//!   programs belong on the bar.
//! * **right** - the tray, exactly as in Windows: an overflow chevron, network,
//!   volume, input language, the clock with a calendar flyout, and the
//!   notification centre.  Each opens a flyout (see `tray.rs`).
//!
//! Application buttons use the icon the window itself reports
//! (`shell::window_icon_rgba`), so an entry never renders as an empty square.

use eframe::egui;
use egui::{Color32, RichText};
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::tray;
use crate::DesktopApp;

pub const HEIGHT: f32 = 46.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEntry {
    pub title: String,
    pub pid: u32,
    pub hwnd: isize,
    /// Executable path, used for the fallback icon.
    pub exe: String,
}

/// List windows through `EnumWindows`, with the owning process' image path so
/// the button can fall back to the executable's icon.
pub fn windows() -> Vec<WindowEntry> {
    winrust_core::shell::top_level_windows()
        .into_iter()
        .filter(|w| !w.title.is_empty())
        // The shell's own window is the desktop; it must not appear on the bar.
        .filter(|w| w.pid != std::process::id())
        .map(|w| WindowEntry {
            title: w.title,
            pid: w.pid,
            hwnd: w.hwnd,
            exe: w.exe,
        })
        .collect()
}

/// `HH:MM`, plus the long date - the tray draws them on two lines.
pub fn clock_text() -> String {
    winrust_core::system::local_time_text()
}

impl DesktopApp {
    pub fn taskbar(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let bar = egui::Rect::from_min_max(
            egui::pos2(screen.left(), screen.bottom() - HEIGHT),
            screen.right_bottom(),
        );
        let pal = winrust_ui::theme::pal(&self.appearance);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("taskbar_bg"),
        ));
        let bar_fill = Color32::from_rgba_unmultiplied(
            pal.surface.0.saturating_sub(8),
            pal.surface.1.saturating_sub(8),
            pal.surface.2.saturating_sub(6),
            246,
        );
        painter.rect_filled(bar, 0.0, bar_fill);
        painter.line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
            ),
        );

        // ---- Left: the Start button only -----------------------------------
        let inner = bar.shrink2(egui::vec2(6.0, 4.0));
        let mut left_ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("taskbar_left")),
            egui::Id::new("taskbar_left_ui"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    inner.left_top(),
                    egui::vec2(48.0, inner.height()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let start_resp = left_ui.add_sized(
            [40.0, 38.0],
            egui::Button::new(RichText::new("⊞").size(20.0))
                .frame(false)
                .fill(if self.start_open {
                    Color32::from_rgba_unmultiplied(
                        pal.selection.0,
                        pal.selection.1,
                        pal.selection.2,
                        150,
                    )
                } else {
                    Color32::TRANSPARENT
                }),
        );
        if start_resp.clicked() {
            self.start_open = !self.start_open;
            self.tray_flyout = tray::Flyout::None;
            if !self.start_open {
                self.start_search.clear();
            }
        }
        // Win+X: the power-user menu lives on a right click, as in Windows.
        start_resp.context_menu(|ui| {
            ui.set_min_width(250.0);
            for (label, action) in power_menu() {
                if ui.button(t(label)).clicked() {
                    action(self);
                    ui.close_menu();
                }
            }
        });
        start_resp.on_hover_text(t("Start"));

        // ---- Running programs ----------------------------------------------
        // One button per window, grouped by process so two windows of the same
        // program share one button, like the real taskbar.
        let windows_list: Vec<WindowEntry> = self.open_windows.clone();
        let mut order: Vec<u32> = Vec::new();
        for w in &windows_list {
            if !order.contains(&w.pid) {
                order.push(w.pid);
            }
        }
        let button_w = 40.0;
        let gap = 2.0;
        let apps_left = inner.left() + 48.0;
        let apps_right = inner.right() - tray::WIDTH;
        let mut apps_ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("taskbar_apps")),
            egui::Id::new("taskbar_apps_ui"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(apps_left, inner.top()),
                    egui::pos2(apps_right.max(apps_left), inner.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        apps_ui.spacing_mut().item_spacing.x = gap;
        apps_ui.horizontal(|ui| {
            for pid in order {
                let first = windows_list.iter().find(|w| w.pid == pid);
                let Some(first) = first else { continue };
                let count = windows_list.iter().filter(|w| w.pid == pid).count();
                let icon = self.window_icon(ctx, first);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(button_w, 38.0), egui::Sense::click());
                let selected = self.active_pid == Some(pid);
                if resp.hovered() || selected {
                    ui.painter().rect_filled(
                        rect.shrink(1.0),
                        5.0,
                        Color32::from_rgba_unmultiplied(
                            pal.hover.0,
                            pal.hover.1,
                            pal.hover.2,
                            if selected { 150 } else { 90 },
                        ),
                    );
                }
                let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(22.0, 22.0));
                match icon {
                    Some(id) => {
                        ui.painter().image(
                            id,
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph_for_title(&first.title),
                            egui::FontId::proportional(17.0),
                            Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
                        );
                    }
                }
                // The running underline: one segment per window.
                let segments = count.min(4) as f32;
                let total_w = 14.0;
                let seg_w = (total_w - (segments - 1.0) * 2.0) / segments;
                for i in 0..count.min(4) {
                    let x = rect.center().x - total_w / 2.0 + i as f32 * (seg_w + 2.0);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, rect.bottom() - 3.0),
                            egui::vec2(seg_w, 3.0),
                        ),
                        1.5,
                        if selected {
                            Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2)
                        } else {
                            Color32::from_rgba_unmultiplied(
                                pal.accent.0,
                                pal.accent.1,
                                pal.accent.2,
                                170,
                            )
                        },
                    );
                }
                if resp.clicked() {
                    self.active_pid = Some(pid);
                    winrust_core::shell::activate_window(pid);
                }
                let hover = if count > 1 {
                    format!("{}\n{}", first.title, tf!("{} windows", count))
                } else {
                    first.title.clone()
                };
                resp.on_hover_text(hover);
            }
            // Nothing running: say so quietly instead of leaving a void.
            if order_empty(&self.open_windows) {
                ui.label(RichText::new(ts("No open windows")).small().weak().color(
                    Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2),
                ));
            }
        });

        // ---- Right: the tray -------------------------------------------------
        tray::draw(self, ctx, inner);
    }

    /// Cached icon for one window: the window's own icon, then its executable's.
    pub(crate) fn window_icon(
        &mut self,
        ctx: &egui::Context,
        w: &WindowEntry,
    ) -> Option<egui::TextureId> {
        if let Some(slot) = self.window_icons.get(&w.pid) {
            return slot.as_ref().map(|t| t.id());
        }
        let rgba = winrust_core::shell::window_icon_rgba(w.hwnd, 32)
            .or_else(|| winrust_core::shell::exe_icon_rgba(&w.exe, 32));
        let texture = rgba.and_then(|rgba| {
            let px = match rgba.len() / 4 {
                n if n >= 48 * 48 => 48,
                n if n >= 32 * 32 => 32,
                n if n >= 16 * 16 => 16,
                _ => return None,
            };
            let expected = px * px * 4;
            if rgba.len() < expected {
                return None;
            }
            Some(ctx.load_texture(
                format!("winicon:{}", w.pid),
                egui::ColorImage::from_rgba_unmultiplied([px, px], &rgba[..expected]),
                egui::TextureOptions::LINEAR,
            ))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.window_icons.insert(w.pid, texture);
        id
    }
}

fn order_empty(list: &[WindowEntry]) -> bool {
    list.is_empty()
}

/// The Start button's right-click menu (Win+X).
fn power_menu() -> Vec<(&'static str, fn(&mut DesktopApp))> {
    vec![
        ("Task Manager", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-taskmgr.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Settings", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("WinRust.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Explorer", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-explorer.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Desktop", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-desktop.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("System Control", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-sysctl.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Services", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-services.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Power", |app: &mut DesktopApp| {
            app.start_open = true;
            app.start_power_menu = true;
        }),
        ("Lock", |_app: &mut DesktopApp| {
            winrust_core::system::lock_workstation();
        }),
        ("Sign out", |_app: &mut DesktopApp| {
            winrust_core::system::sign_out();
        }),
        ("Shut down", |_app: &mut DesktopApp| {
            winrust_core::system::shutdown();
        }),
        ("Restart", |_app: &mut DesktopApp| {
            winrust_core::system::restart();
        }),
    ]
}

/// A glyph for a window whose icon could not be read at all.
fn glyph_for_title(title: &str) -> &'static str {
    let t = title.to_lowercase();
    if t.contains("проводник") || t.contains("explorer") {
        "🗂"
    } else if t.contains("диспетчер") || t.contains("task manager") {
        "📊"
    } else if t.contains("служб") || t.contains("services") {
        "⚙"
    } else {
        "🪟"
    }
}

/// Windows to offer in the Start menu's "running" section.
pub fn describe_running(list: &[WindowEntry]) -> String {
    if list.is_empty() {
        ts("No open windows").to_string()
    } else {
        tf!("{} open windows", list.len())
    }
}

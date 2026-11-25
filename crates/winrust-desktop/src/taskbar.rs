//! The taskbar strip: Start button, running windows, clock and system status.
//!
//! A shell is judged by whether the user can get back to their programs, so the
//! taskbar lists every top-level window and brings it to the foreground on
//! click, instead of pretending windows do not exist.

use eframe::egui;
use egui::{Color32, RichText};
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::DesktopApp;

pub const HEIGHT: f32 = 46.0;

#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub title: String,
    pub pid: u32,
}

/// List windows through `EnumWindows`; the launcher's `windows` helper already
/// wraps this, so the shell only formats the result.
pub fn windows() -> Vec<WindowEntry> {
    winrust_core::shell::top_level_windows()
        .into_iter()
        .filter(|w| !w.title.is_empty())
        .map(|w| WindowEntry {
            title: w.title,
            pid: w.pid,
        })
        .collect()
}

/// `HH:MM` in the language's conventions, plus the date from the system clock.
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
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("taskbar_bg"),
        ));
        // The strip follows the shared theme, like every other panel.
        let pal = winrust_ui::theme::pal(&self.appearance);
        let bar_fill = Color32::from_rgba_unmultiplied(
            pal.surface.0.saturating_sub(8),
            pal.surface.1.saturating_sub(8),
            pal.surface.2.saturating_sub(6),
            240,
        );
        painter.rect_filled(bar, 0.0, bar_fill);
        painter.line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
            ),
        );

        // ---- The centred icon run, the way Windows 11 shows it: Start, the
        // pinned tools, then one button per open window.  Every button is the
        // same width, so the run's width is known in advance and the whole strip
        // can be positioned before anything is drawn.
        let inner = bar.shrink2(egui::vec2(6.0, 5.0));
        let left_width = 220.0;
        let right_width = 300.0;
        let centre_area = egui::Rect::from_min_max(
            egui::pos2(inner.left() + left_width, inner.top()),
            egui::pos2(inner.right() - right_width, inner.bottom()),
        );

        // Every icon is the same size, so the run's width is predictable: the
        // Start button plus one 40 px slot per entry plus separators.
        let mut slots: Vec<TaskButton> = Vec::new();
        slots.push(TaskButton::Start);
        for (glyph, exe, label) in PINNED {
            slots.push(TaskButton::Pinned { glyph, exe, label });
        }
        for w in self.open_windows.clone() {
            slots.push(TaskButton::Window(w));
        }

        let button_w = 40.0;
        let gap = 4.0;
        let total = slots.len() as f32 * (button_w + gap) - gap;
        let centre_x = centre_area.center().x - total / 2.0;

        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("taskbar_layer")),
            egui::Id::new("taskbar"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(centre_x.max(inner.left()), inner.top()),
                    egui::vec2(inner.right() - centre_x.max(inner.left()), inner.height()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = gap;
        ui.scope(|ui| {
            for slot in &slots {
                match slot {
                    TaskButton::Start => {
                        let open = self.start_open;
                        let resp = ui.add_sized(
                            [button_w, 36.0],
                            egui::Button::new(RichText::new("⊞").size(19.0))
                                .frame(false)
                                .fill(if open {
                                    Color32::from_rgba_unmultiplied(
                                        pal.selection.0,
                                        pal.selection.1,
                                        pal.selection.2,
                                        170,
                                    )
                                } else {
                                    Color32::TRANSPARENT
                                }),
                        );
                        if resp.clicked() {
                            self.start_open = !self.start_open;
                            if !self.start_open {
                                self.start_search.clear();
                            }
                        }
                        resp.on_hover_text(t("Start"));
                    }
                    TaskButton::Pinned { glyph, exe, label } => {
                        let resp = ui.add_sized(
                            [button_w, 36.0],
                            egui::Button::new(RichText::new(*glyph).size(18.0)).frame(false),
                        );
                        if resp.clicked() {
                            if let Some(path) = winrust_core::install::find_binary(exe) {
                                let _ = winrust_core::shell::execute(&path, "", None);
                            } else {
                                self.status = tf!("{} not found", exe);
                            }
                        }
                        resp.on_hover_text(t(label));
                    }
                    TaskButton::Window(w) => {
                        let running = w.pid != 0;
                        let resp = ui.add_sized(
                            [button_w, 36.0],
                            egui::Button::new(RichText::new(icon_for_title(&w.title)).size(17.0))
                                .frame(false),
                        );
                        if resp.clicked() {
                            winrust_core::shell::activate_window(w.pid);
                        }
                        // `Response` is not `Copy`: take the rect first, then hand
                        // the response to `on_hover_text`, which consumes it.
                        let r = resp.rect;
                        resp.on_hover_text(format!(
                            "{}\n{} · PID {}",
                            w.title,
                            t("Running"),
                            w.pid
                        ));
                        // The signature Windows 11 underline under a live app.
                        if running {
                            let underline = egui::Rect::from_min_size(
                                egui::pos2(r.center().x - 9.0, r.bottom() - 2.0),
                                egui::vec2(18.0, 3.0),
                            );
                            ui.painter().rect_filled(
                                underline,
                                1.5,
                                Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2),
                            );
                        }
                    }
                }
            }
        });

        // ---- Right: the tray.  A separate Ui anchored right to left, so the
        // clock never moves when the window list grows.
        let mut tray = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tray_layer")),
            egui::Id::new("tray"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(inner.right() - right_width.min(inner.width()), inner.top()),
                    egui::vec2(right_width.min(inner.width()), inner.height()),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        tray.spacing_mut().item_spacing.x = 8.0;
        tray.scope(|ui| {
            // Clock, two lines, exactly like the real tray.
            let (time, date) = split_clock(&self.clock);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(time).size(12.5));
                ui.label(RichText::new(date).small().weak());
            });
            ui.separator();
            let (cpu, mem) = self.cpu_mem;
            ui.label(
                RichText::new(tf!(
                    "CPU {}% · RAM {}%",
                    format_args!("{:.0}", cpu),
                    format_args!("{:.0}", mem.used_percent())
                ))
                .small()
                .weak(),
            )
            .on_hover_text(t("Live CPU and memory use, sampled once a second"));
            if !self.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.status).small().weak())
                    .on_hover_text(&self.status);
            }
            // The tray chevron opens the power menu, like the real overflow.
            if ui
                .add(egui::Button::new(RichText::new("⏻").size(14.0)).frame(false))
                .on_hover_text(t("Power"))
                .clicked()
            {
                self.start_open = true;
                self.start_power_menu = true;
            }
        });
    }
}

/// The pinned apps, in the order Windows 11 shows them.
const PINNED: [(&str, &str, &str); 4] = [
    ("🗂", "winrust-explorer.exe", "Explorer"),
    ("📊", "winrust-taskmgr.exe", "Task Manager"),
    ("⚙", "winrust-services.exe", "Services"),
    ("🛠", "winrust-sysctl.exe", "System Control"),
];

/// One slot of the centred icon run.
enum TaskButton {
    Start,
    Pinned {
        glyph: &'static str,
        exe: &'static str,
        label: &'static str,
    },
    Window(WindowEntry),
}

/// `"16:44 · 25.11.2026"` -> `("16:44", "25.11.2026")`.
fn split_clock(text: &str) -> (String, String) {
    match text.split_once(" · ") {
        Some((t, d)) => (t.to_string(), d.to_string()),
        None => (text.to_string(), String::new()),
    }
}

/// A glyph for a window entry: Explorer-like guess from the executable name,
/// falling back to a generic window icon.
fn icon_for_title(title: &str) -> &'static str {
    let t = title.to_lowercase();
    if t.contains("проводник") || t.contains("explorer") || t.contains("file explorer") {
        "🗂"
    } else if t.contains("диспетчер") || t.contains("task manager") {
        "📊"
    } else if t.contains("служб") || t.contains("services") {
        "⚙"
    } else if t.contains("браузер") || t.contains("chrome") || t.contains("edge") {
        "🌐"
    } else if t.contains("word") || t.contains("excel") || t.contains("office") {
        "📄"
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

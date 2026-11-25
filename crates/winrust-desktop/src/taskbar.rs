//! The taskbar strip: Start button, running windows, clock and system status.
//!
//! A shell is judged by whether the user can get back to their programs, so the
//! taskbar lists every top-level window and brings it to the foreground on
//! click, instead of pretending windows do not exist.

use eframe::egui;
use egui::{Color32, RichText};
use std::path::PathBuf;
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::DesktopApp;

pub const HEIGHT: f32 = 46.0;

#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub title: String,
    pub pid: u32,
    pub exe: PathBuf,
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
            exe: PathBuf::new(),
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
        painter.rect_filled(bar, 0.0, Color32::from_rgba_unmultiplied(16, 20, 28, 235));
        painter.line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(1.0_f32, Color32::from_rgb(44, 52, 66)),
        );

        let ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("taskbar"),
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(egui::vec2(6.0, 5.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // Start
            let start_selected = self.start_open;
            let start = ui.add_sized(
                [78.0, 34.0],
                egui::SelectableLabel::new(
                    start_selected,
                    RichText::new(format!("⊞ {}", t("Start"))).size(14.0),
                ),
            );
            if start.clicked() {
                self.start_open = !self.start_open;
                if !self.start_open {
                    self.start_search.clear();
                }
            }
            start.on_hover_text(t("Start menu"));

            // A few pinned tools: Explorer, Task Manager, Services, Desktop.
            let pinned: [(&str, &str, &str); 4] = [
                ("🗂", "winrust-explorer.exe", "Explorer"),
                ("📊", "winrust-taskmgr.exe", "Task Manager"),
                ("⚙", "winrust-services.exe", "Services"),
                ("🛠", "winrust-sysctl.exe", "System Control"),
            ];
            for (glyph, exe, label) in pinned {
                if ui
                    .add_sized(
                        [34.0, 34.0],
                        egui::Button::new(RichText::new(glyph).size(16.0)).frame(false),
                    )
                    .on_hover_text(t(label))
                    .clicked()
                {
                    if let Some(path) = winrust_core::install::find_binary(exe) {
                        let _ = winrust_core::shell::execute(&path, "", None);
                    } else {
                        self.status = tf!("{} not found", exe);
                    }
                }
            }

            ui.separator();

            // Running windows.
            egui::ScrollArea::horizontal()
                .id_salt("taskbar_windows")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for w in self.open_windows.clone() {
                            let label = winrust_core::i18n::end(&w.title, 26);
                            if ui
                                .add_sized(
                                    [150.0, 34.0],
                                    egui::Button::new(RichText::new(label).size(12.5)).frame(true),
                                )
                                .on_hover_text(format!("{} · PID {}", w.title, w.pid))
                                .clicked()
                            {
                                winrust_core::shell::activate_window(w.pid);
                            }
                        }
                    });
                });

            // Right side: status + clock.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(&self.clock).size(13.0).strong());
                // Taskbar status is sampled in the shell's own tick, not here:
                // two calls one microsecond apart would always read 0 %.
                let (cpu, mem) = self.cpu_mem;
                ui.label(
                    RichText::new(tf!(
                        "CPU {}% · RAM {}%",
                        format_args!("{:.0}", cpu),
                        format_args!("{:.0}", mem.used_percent())
                    ))
                    .small()
                    .weak(),
                );
                if ui
                    .add(egui::Button::new(RichText::new("⏻").size(15.0)).frame(false))
                    .on_hover_text(t("Power"))
                    .clicked()
                {
                    self.start_open = true;
                }
            });
        });
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

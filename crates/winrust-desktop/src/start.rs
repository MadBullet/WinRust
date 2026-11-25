//! The Start menu: search, apps, power and the way back to the stock shell.

use eframe::egui;
use egui::{Color32, RichText};
use std::path::PathBuf;
use winrust_core::i18n::ts;
use winrust_core::shell::KnownFolder;
use winrust_core::{t, tf};

use crate::taskbar::describe_running;
use crate::DesktopApp;

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Installed applications: Start Menu shortcuts (`*.lnk`) from both the user
/// and machine-wide folders.  This is the same list the stock Start menu reads.
pub fn installed_apps() -> Vec<AppEntry> {
    let mut out: Vec<AppEntry> = Vec::new();
    let roots = [
        KnownFolder::RoamingAppData
            .path()
            .map(|p| p.join("Microsoft\\Windows\\Start Menu\\Programs")),
        std::env::var_os("ProgramData")
            .map(|p| PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs")),
    ];
    for root in roots.into_iter().flatten() {
        walk(&root, &mut out, 0);
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    // WinRust's own programs first, so a freshly installed suite is usable
    // without hunting through the Start Menu tree.
    let mut ours: Vec<AppEntry> = [
        "winrust-explorer.exe",
        "winrust-taskmgr.exe",
        "winrust-services.exe",
        "winrust-sysctl.exe",
    ]
    .iter()
    .filter_map(|exe| {
        winrust_core::install::find_binary(exe).map(|path| AppEntry {
            name: exe.trim_end_matches(".exe").to_string(),
            path,
        })
    })
    .collect();
    ours.extend(out);
    ours
}

fn walk(dir: &std::path::Path, out: &mut Vec<AppEntry>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out, depth + 1);
        } else if path
            .extension()
            .map(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url"))
            .unwrap_or(false)
        {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                out.push(AppEntry { name, path });
            }
        }
    }
}

impl DesktopApp {
    pub fn start_menu(&mut self, ctx: &egui::Context) {
        if !self.start_open {
            return;
        }
        let screen = ctx.screen_rect();
        // Above the taskbar, anchored bottom-left like Windows 10.
        let width = 420.0_f32.min(screen.width() - 40.0);
        let height = 460.0_f32.min(screen.height() - 120.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(screen.left() + 8.0, screen.bottom() - 46.0 - height - 8.0),
            egui::vec2(width, height),
        );
        egui::Area::new(egui::Id::new("start_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top())
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(24, 28, 38, 248))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(56, 64, 82)))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_width(rect.width() - 24.0);
                        ui.set_height(rect.height() - 20.0);

                        // Search.  The hint doubles as the tooltip on the
                        // field itself, since the placeholder may be cut short.
                        ui.add_sized(
                            [ui.available_width(), 26.0],
                            egui::TextEdit::singleline(&mut self.start_search)
                                .hint_text(format!("🔍 {}", t("Search apps"))),
                        )
                        .on_hover_text(hint_text());
                        ui.add_space(4.0);

                        let needle = self.start_search.to_lowercase();
                        let mut launch: Option<PathBuf> = None;
                        let mut nav: Option<PathBuf> = None;
                        let mut switched = false;

                        // Known folders row.
                        ui.horizontal(|ui| {
                            for folder in [
                                KnownFolder::Profile,
                                KnownFolder::Documents,
                                KnownFolder::Downloads,
                                KnownFolder::Pictures,
                                KnownFolder::Computer,
                            ] {
                                if ui
                                    .add(egui::Button::new(
                                        RichText::new(folder.label()).small(),
                                    ))
                                    .clicked()
                                {
                                    if let Some(p) = folder.path() {
                                        nav = Some(p);
                                    }
                                }
                            }
                        });
                        ui.separator();

                        egui::ScrollArea::vertical()
                            .id_salt("start_apps")
                            .auto_shrink([false, false])
                            .max_height(rect.height() - 170.0)
                            .show(ui, |ui| {
                                let mut shown = 0usize;
                                for app in self.apps.iter() {
                                    if !needle.is_empty()
                                        && !app.name.to_lowercase().contains(&needle)
                                    {
                                        continue;
                                    }
                                    shown += 1;
                                    if ui
                                        .add_sized(
                                            [ui.available_width(), 22.0],
                                            egui::Button::new(RichText::new(&app.name).size(12.5))
                                                .frame(false),
                                        )
                                        .clicked()
                                    {
                                        launch = Some(app.path.clone());
                                    }
                                }
                                if shown == 0 {
                                    ui.label(RichText::new(ts("Nothing found")).weak().small());
                                }
                            });

                        // Running windows, so the Start menu doubles as the
                        // task switcher the real Start menu's app list is.
                        ui.separator();
                        ui.label(RichText::new(describe_running(&self.open_windows)).small().weak());
                        egui::ScrollArea::vertical()
                            .id_salt("start_windows")
                            .auto_shrink([false, false])
                            .max_height(96.0)
                            .show(ui, |ui| {
                                for w in self.open_windows.iter().take(12) {
                                    if ui
                                        .add_sized(
                                            [ui.available_width(), 22.0],
                                            egui::Button::new(
                                                RichText::new(
                                                    winrust_core::i18n::end(&w.title, 40),
                                                )
                                                .size(12.5),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        winrust_core::shell::activate_window(w.pid);
                                        switched = true;
                                    }
                                }
                            });

                        ui.separator();
                        // Power and the escape hatch back to the stock shell.
                        ui.horizontal(|ui| {
                            if ui.button(format!("⏻ {}", t("Sign out"))).clicked() {
                                winrust_core::system::sign_out();
                            }
                            if ui.button(t("Lock")).clicked() {
                                winrust_core::system::lock_workstation();
                            }
                            if ui.button(t("Restart")).clicked() {
                                winrust_core::system::restart();
                            }
                            if ui.button(t("Shut down")).clicked() {
                                winrust_core::system::shutdown();
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button(t("Back to Windows shell"))
                                        .on_hover_text(t(
                                            "Removes the Winlogon Shell override and starts explorer.exe",
                                        ))
                                        .clicked()
                                    {
                                        match winrust_core::install::set_shell(None) {
                                            Ok(()) => {
                                                let _ = winrust_core::shell::execute(
                                                    std::path::Path::new("explorer.exe"),
                                                    "",
                                                    None,
                                                );
                                                self.status =
                                                    ts("Windows shell restored").to_string();
                                            }
                                            Err(e) => {
                                                self.status =
                                                    tf!("Could not restore the shell: {}", e)
                                            }
                                        }
                                    }
                                },
                            );
                        });

                        if switched {
                            // Bringing a window forward is not navigation, but
                            // the Start menu should still get out of the way.
                            self.close_start();
                        }
                        if let Some(path) = launch {
                            self.close_start();
                            self.open_path(&path);
                        }
                        if let Some(path) = nav {
                            self.close_start();
                            self.open_path(&path);
                        }
                    });
            });
        // Clicking outside closes the menu.
        if ctx.input(|i| i.pointer.any_click()) {
            let pos = ctx.input(|i| i.pointer.latest_pos());
            if let Some(p) = pos {
                if !rect.contains(p) {
                    self.close_start();
                }
            }
        }
    }
}

/// The status line under the Start search results.
pub fn hint_text() -> &'static str {
    ts("Type to search installed apps and folders")
}

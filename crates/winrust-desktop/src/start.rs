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
        // Above the taskbar and horizontally centred on it, like Windows 11.
        let width = 560.0_f32.min(screen.width() - 40.0);
        let height = 560.0_f32.min(screen.height() - 140.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                (screen.center().x - width / 2.0).max(screen.left() + 8.0),
                screen.bottom() - crate::taskbar::HEIGHT - height - 8.0,
            ),
            egui::vec2(width, height),
        );
        let pal = winrust_ui::theme::pal(&self.appearance);
        egui::Area::new(egui::Id::new("start_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top())
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(
                        pal.surface.0,
                        pal.surface.1,
                        pal.surface.2,
                        252,
                    ))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
                    ))
                    .rounding(10.0)
                    .inner_margin(egui::Margin::same(14.0))
                    .show(ui, |ui| {
                        ui.set_width(rect.width() - 28.0);
                        ui.set_height(rect.height() - 20.0);

                        // Header: the "ПУСК" caption and the account button.
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t("START").to_uppercase())
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(pal.weak.0, pal.weak.1, pal.weak.2)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let user = winrust_core::system::user_display_name();
                                    ui.menu_button(
                                        RichText::new(format!("{}  👤", user)).size(12.5),
                                        |ui| {
                                            ui.set_min_width(240.0);
                                            if ui
                                                .button(t("Change account settings"))
                                                .clicked()
                                            {
                                                let _ = winrust_core::shell::launch(
                                                    "rundll32.exe",
                                                    "shell32.dll,Control_RunDLL userpasswords",
                                                );
                                                ui.close_menu();
                                            }
                                            if ui.button(t("Lock")).clicked() {
                                                winrust_core::system::lock_workstation();
                                                ui.close_menu();
                                            }
                                            if ui.button(t("Sign out")).clicked() {
                                                winrust_core::system::sign_out();
                                                ui.close_menu();
                                            }
                                            if ui.button(t("Switch user")).clicked() {
                                                let _ = winrust_core::shell::launch(
                                                    "tsdiscon.exe",
                                                    "",
                                                );
                                                ui.close_menu();
                                            }
                                        },
                                    );
                                },
                            );
                        });
                        ui.add_space(2.0);

                        // Search.
                        ui.add_sized(
                            [ui.available_width(), 30.0],
                            egui::TextEdit::singleline(&mut self.start_search)
                                .hint_text(format!("🔍 {}", t("Search apps"))),
                        )
                        .on_hover_text(hint_text());
                        ui.add_space(6.0);

                        let needle = self.start_search.to_lowercase();
                        let mut launch: Option<PathBuf> = None;
                        let mut nav: Option<PathBuf> = None;
                        let mut switched = false;

                        // Tiles grid, Windows 11 style: six per row.
                        let footer = 96.0;
                        let tiles_h = rect.height() - footer;
                        egui::ScrollArea::vertical()
                            .id_salt("start_apps")
                            .auto_shrink([false, false])
                            .max_height(tiles_h)
                            .show(ui, |ui| {
                                let tile_w = (ui.available_width() - 12.0) / 3.0;
                                let mut shown = 0usize;
                                egui::Grid::new("start_tiles")
                                    .num_columns(3)
                                    .spacing(egui::vec2(6.0, 6.0))
                                    .show(ui, |ui| {
                                        let mut col = 0;
                                        for app in self.apps.iter() {
                                            if !needle.is_empty()
                                                && !app.name.to_lowercase().contains(&needle)
                                            {
                                                continue;
                                            }
                                            shown += 1;
                                            let resp = ui.add_sized(
                                                [tile_w, 38.0],
                                                egui::Button::new(
                                                    RichText::new(
                                                        winrust_core::i18n::end(&app.name, 22),
                                                    )
                                                    .size(12.0),
                                                )
                                                .truncate()
                                                .fill(Color32::from_rgba_unmultiplied(
                                                    pal.tile.0, pal.tile.1, pal.tile.2, 255,
                                                )),
                                            );
                                            if resp.clicked() {
                                                launch = Some(app.path.clone());
                                            }
                                            col += 1;
                                            if col == 3 {
                                                ui.end_row();
                                                col = 0;
                                            }
                                        }
                                        if col != 0 {
                                            ui.end_row();
                                        }
                                    });
                                if shown == 0 {
                                    ui.label(RichText::new(ts("Nothing found")).weak().small());
                                }
                            });

                        // Footer: user row -> Настройки -> Выключение.
                        ui.separator();
                        ui.horizontal(|ui| {
                            // Known folders as a compact row.
                            for folder in [
                                KnownFolder::Profile,
                                KnownFolder::Documents,
                                KnownFolder::Downloads,
                                KnownFolder::Computer,
                            ] {
                                if ui
                                    .add(egui::Button::new(RichText::new(folder.label()).small()).frame(false))
                                    .clicked()
                                {
                                    if let Some(p) = folder.path() {
                                        nav = Some(p);
                                    }
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui
                                .button(format!("⚙ {}", t("Settings")))
                                .on_hover_text(t("Open the WinRust launcher's settings page"))
                                .clicked()
                            {
                                if let Some(path) =
                                    winrust_core::install::find_binary("winrust-launcher.exe")
                                {
                                    let _ = winrust_core::shell::execute(&path, "--settings", None);
                                }
                            }
                            // Power: a menu, so nothing is one misclick away.
                            ui.menu_button(format!("⏻ {}", t("Power")), |ui| {
                                if ui.button(t("Sleep")).clicked() {
                                    let _ = winrust_core::shell::launch("rundll32.exe", "powrprof.dll,SetSuspendState 0,1,0");
                                    ui.close_menu();
                                }
                                if ui.button(t("Shut down")).clicked() {
                                    winrust_core::system::shutdown();
                                    ui.close_menu();
                                }
                                if ui.button(t("Restart")).clicked() {
                                    winrust_core::system::restart();
                                    ui.close_menu();
                                }
                                if ui.button(t("Lock")).clicked() {
                                    winrust_core::system::lock_workstation();
                                    ui.close_menu();
                                }
                            });
                            if self.start_power_menu {
                                ui.label(
                                    RichText::new(t("Power options are above"))
                                        .small()
                                        .weak(),
                                );
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
                                    ui.label(
                                        RichText::new(tf!(
                                            "{} open",
                                            self.open_windows.len()
                                        ))
                                        .small()
                                        .weak(),
                                    )
                                    .on_hover_text(describe_running(&self.open_windows));
                                },
                            );
                        });
                        // The running-window switcher lives under the tiles so the
                        // footer stays as short as Windows' own.
                        egui::CollapsingHeader::new(t("Running windows"))
                            .id_salt("start_running")
                            .default_open(false)
                            .show(ui, |ui| {
                                for w in self.open_windows.iter().take(12) {
                                    if ui
                                        .add_sized(
                                            [ui.available_width(), 22.0],
                                            egui::Button::new(
                                                RichText::new(
                                                    winrust_core::i18n::end(&w.title, 46),
                                                )
                                                .size(12.0),
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

                        if switched {
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
                    self.start_power_menu = false;
                }
            }
        }
        if !self.start_open {
            self.start_power_menu = false;
        }
    }
}

/// The status line under the Start search results.
pub fn hint_text() -> &'static str {
    ts("Type to search installed apps and folders")
}

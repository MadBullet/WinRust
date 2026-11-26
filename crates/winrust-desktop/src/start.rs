//! The Start menu: a list of every app on the left, pinned tiles on the
//! right, and the account / Settings / power row at the bottom.
//!
//! The layout is Windows 11's, and the split is what the 0.9.1 review asked
//! for: the old menu was a single six-per-row grid of text buttons, which
//! turned "find an application" into reading 60 identical rectangles.  Here
//!
//! * the **left** column lists every installed app once, alphabetically, with
//!   the icon the shell reports for it, and a search box on top;
//! * the **right** column is the pinned grid: square tiles with the icon and
//!   the name under it, which is the part of the menu a user actually clicks
//!   without reading;
//! * the **bottom** row holds exactly three things - Settings, the current
//!   user, and the power button - and the power button opens a list, not a
//!   menu that fires on hover.  Every power entry asks for a confirmation.
//!
//! Geometry: the panel is anchored 8 px above the taskbar and is 560 px tall
//! at most.  The 0.9.0 menu computed the height from the *screen*, which put
//! its footer under the taskbar on small displays and made the last row
//! unreachable - the "sits too low / footer clipped" defect.  It is now
//! anchored bottom-up to the bar's top edge, so the footer is always above
//! the bar.

use eframe::egui;
use egui::{Color32, RichText};
use std::path::PathBuf;
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::session::{AppEntry, PowerAction};
use crate::DesktopApp;

/// Installed applications: Start Menu shortcuts (`*.lnk`) from both the user
/// and machine-wide folders.  This is the same list the stock Start menu reads.
pub fn installed_apps() -> Vec<AppEntry> {
    let mut out: Vec<AppEntry> = Vec::new();
    let roots = [
        winrust_core::shell::KnownFolder::RoamingAppData
            .path()
            .map(|p| p.join("Microsoft\\Windows\\Start Menu\\Programs")),
        std::env::var_os("ProgramData")
            .map(|p| PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs")),
    ];
    for root in roots.into_iter().flatten() {
        walk(&root, &mut out, 0);
    }
    // One entry per name, and alphabetical - the order the left column shows.
    // The 0.9.0 list prepended WinRust's own exes *after* sorting, which put
    // four entries out of order at the top of an otherwise sorted list.
    for (name, exe) in [
        ("WinRust Explorer", "winrust-explorer.exe"),
        ("WinRust Task Manager", "winrust-taskmgr.exe"),
        ("WinRust Services", "winrust-services.exe"),
        ("WinRust System Control", "winrust-sysctl.exe"),
        ("WinRust Launcher", "WinRust.exe"),
    ] {
        if let Some(path) = winrust_core::install::find_binary(exe) {
            out.push(AppEntry {
                name: t(name),
                path,
            });
        }
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.path.cmp(&b.path))
    });
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
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

/// The applications the pinned grid shows when the user has not chosen any.
///
/// Windows pins what its vendors install; a bare Windows install has no
/// `StartMenu` pin database we can read without COM, so the shell pins its own
/// programs plus the handful of things Windows always ships.  The point of the
/// grid is to be clickable without reading, so these are real paths, filtered
/// to the ones that exist.
fn default_pins() -> Vec<AppEntry> {
    let mut pins: Vec<AppEntry> = Vec::new();
    for (name, exe) in [
        ("WinRust Explorer", "winrust-explorer.exe"),
        ("WinRust Task Manager", "winrust-taskmgr.exe"),
        ("WinRust Services", "winrust-services.exe"),
        ("WinRust System Control", "winrust-sysctl.exe"),
        ("WinRust Launcher", "WinRust.exe"),
    ] {
        if let Some(path) = winrust_core::install::find_binary(exe) {
            pins.push(AppEntry {
                name: t(name),
                path,
            });
        }
    }
    for (name, cmd) in [
        ("Notepad", "notepad.exe"),
        ("Paint", "mspaint.exe"),
        ("Calculator", "calc.exe"),
        ("Command Prompt", "cmd.exe"),
        ("Control Panel", "control.exe"),
        ("Task Manager", "taskmgr.exe"),
    ] {
        if let Some(path) = winrust_core::install::find_binary(cmd) {
            pins.push(AppEntry {
                name: t(name),
                path,
            });
        }
    }
    pins
}

impl DesktopApp {
    pub fn start_menu(&mut self, ctx: &egui::Context) {
        // Drawn while the animation is closing too, so it slides away instead
        // of vanishing on the frame `open` turns false.
        if !self.session.visible() {
            return;
        }
        let anim = self.session.anim().clamp(0.0, 1.0);
        let screen = ctx.screen_rect();
        // Above the taskbar, centred, never taller than what is left of the
        // screen.  Anchor from the bar's top edge so the bottom row is always
        // visible - the 0.9.0 menu anchored from the screen bottom and its
        // footer fell behind the bar on short displays.
        let width = 620.0_f32.min(screen.width() - 40.0);
        let max_height = (screen.bottom() - crate::taskbar::HEIGHT - 16.0 - screen.top()).max(280.0);
        let height = 560.0_f32.min(max_height);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                (screen.center().x - width / 2.0).max(screen.left() + 8.0),
                screen.bottom() - crate::taskbar::HEIGHT - 8.0 - height,
            ),
            egui::vec2(width, height),
        );
        let pal = winrust_ui::theme::pal(&self.appearance);
        // Rise 10 px and fade in, like the tray flyouts.
        let slide = (1.0 - anim) * 10.0;
        let alpha = (anim * 250.0) as u8;

        let mut launch: Option<PathBuf> = None;
        let mut ask_power: Option<PowerAction> = None;
        let mut open_settings = false;
        let mut switch_user = false;
        let mut switched_window = false;

        let response = egui::Area::new(egui::Id::new("start_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_top() + egui::vec2(0.0, slide))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(
                        pal.surface.0,
                        pal.surface.1,
                        pal.surface.2,
                        alpha,
                    ))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
                    ))
                    .rounding(10.0)
                    .inner_margin(egui::Margin::same(14.0))
                    .show(ui, |ui| {
                        ui.set_width(rect.width() - 28.0);
                        ui.set_height(rect.height() - 28.0);

                        // ---- Search -------------------------------------------------
                        ui.add_sized(
                            [ui.available_width(), 30.0],
                            egui::TextEdit::singleline(self.session.search_mut())
                                .hint_text(format!("🔍 {}", t("Search apps"))),
                        );
                        ui.add_space(8.0);

                        // ---- Two columns -------------------------------------------
                        // The footer row is reserved first, so the columns get
                        // whatever is left and can never push it out of the
                        // panel.
                        let footer_h = 42.0;
                        let body_h = (ui.available_height() - footer_h).max(120.0);
                        ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                            ui.columns(2, |cols| {
                                let (list, pins) = cols.split_at_mut(1);
                                self.start_app_list(&mut list[0], &mut launch);
                                self.start_pinned_grid(&mut pins[0], ctx, &mut launch);
                            });
                        });

                        ui.add_space(4.0);
                        ui.separator();
                        self.start_footer(
                            ui,
                            &mut ask_power,
                            &mut open_settings,
                            &mut switch_user,
                            &mut switched_window,
                        );
                    });
            })
            .response;

        // Everything the closure decided is carried out here, where `self` is
        // free again: `launch` opens a path, `ask_power` only *asks*.
        if let Some(path) = launch {
            self.session.close();
            self.open_path(&path);
        }
        if open_settings {
            self.session.close();
            // `WinRust.exe --settings`; the launcher is the settings surface.
            if let Some(path) = winrust_core::install::find_binary("WinRust.exe") {
                let _ = winrust_core::shell::execute(&path, "--settings", None);
            }
        }
        if switch_user {
            self.session.close();
            let _ = winrust_core::shell::launch("tsdiscon.exe", "");
        }
        if let Some(action) = ask_power {
            // Hibernation needs the machine to allow it; say so instead of
            // doing nothing.
            if action == PowerAction::Hibernate && !winrust_core::shell::hibernate_allowed() {
                self.status = ts("Hibernation is not enabled on this PC").to_string();
            } else {
                self.session.ask_power(action);
            }
        }
        if switched_window {
            self.session.close();
        }

        // Clicking outside closes the menu.  The check must run *before* the
        // power confirmation is drawn, because opening that confirmation is
        // itself a click outside the menu's rect.
        if self.session.is_open() && ctx.input(|i| i.pointer.any_click()) {
            if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
                if !response.rect.contains(p) {
                    self.session.close();
                }
            }
        }
    }

    /// The left column: every app, alphabetical, icon + name, one per row.
    fn start_app_list(&mut self, ui: &mut egui::Ui, launch: &mut Option<PathBuf>) {
        let pal = winrust_ui::theme::pal(&self.appearance);
        let needle = self.session.search().to_lowercase();
        let apps: Vec<AppEntry> = self.session.apps().to_vec();
        egui::ScrollArea::vertical()
            .id_salt("start_app_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut shown = 0usize;
                for app in &apps {
                    if !needle.is_empty() && !app.name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    shown += 1;
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            5.0,
                            Color32::from_rgba_unmultiplied(
                                pal.hover.0,
                                pal.hover.1,
                                pal.hover.2,
                                90,
                            ),
                        );
                    }
                    // The icon the shell reports for the shortcut; a square
                    // glyph when there is none (a `.url` has no icon file).
                    let slot = format!("start_icon|{}", app.path.display());
                    let icon = self.file_icon_texture(ui.ctx(), slot, || shortcut_icon(&app.path));
                    let icon_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.left() + 18.0, rect.center().y),
                        egui::vec2(18.0, 18.0),
                    );
                    match icon {
                        Some(id) => {
                            ui.painter().image(
                                id,
                                icon_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        None => {
                            ui.painter().text(
                                icon_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "▣",
                                egui::FontId::proportional(15.0),
                                Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2),
                            );
                        }
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 34.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        winrust_core::i18n::end(&app.name, 34),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
                    );
                    if resp.clicked() {
                        *launch = Some(app.path.clone());
                    }
                    resp.on_hover_text(app.path.display().to_string());
                }
                if shown == 0 {
                    ui.add_space(8.0);
                    ui.label(RichText::new(ts("Nothing found")).weak().small());
                }
            });
    }

    /// The right column: pinned apps as square icon tiles with the name below.
    fn start_pinned_grid(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        launch: &mut Option<PathBuf>,
    ) {
        let pal = winrust_ui::theme::pal(&self.appearance);
        let pins = default_pins();
        ui.label(
            RichText::new(t("Pinned").to_uppercase())
                .size(11.0)
                .strong()
                .color(Color32::from_rgb(pal.weak.0, pal.weak.1, pal.weak.2)),
        );
        ui.add_space(6.0);
        let tile = 74.0;
        let gap = 6.0;
        let per_row = ((ui.available_width() + gap) / (tile + gap)).floor().max(1.0) as usize;
        egui::ScrollArea::vertical()
            .id_salt("start_pins")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut chunk_start = 0usize;
                while chunk_start < pins.len() {
                    let chunk_end = (chunk_start + per_row).min(pins.len());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for app in &pins[chunk_start..chunk_end] {
                            let (rect, resp) =
                                ui.allocate_exact_size(egui::vec2(tile, tile + 18.0), egui::Sense::click());
                            let square = egui::Rect::from_min_size(rect.min, egui::vec2(tile, tile));
                            let fill = if resp.hovered() {
                                Color32::from_rgba_unmultiplied(pal.hover.0, pal.hover.1, pal.hover.2, 120)
                            } else {
                                Color32::from_rgba_unmultiplied(pal.tile.0, pal.tile.1, pal.tile.2, 255)
                            };
                            ui.painter().rect_filled(square, 8.0, fill);
                            let slot = format!("pin_icon|{}", app.path.display());
                            let icon =
                                self.file_icon_texture(ctx, slot, || shortcut_icon(&app.path));
                            let icon_rect = egui::Rect::from_center_size(
                                square.center(),
                                egui::vec2(34.0, 34.0),
                            );
                            match icon {
                                Some(id) => {
                                    ui.painter().image(
                                        id,
                                        icon_rect,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        Color32::WHITE,
                                    );
                                }
                                None => {
                                    ui.painter().text(
                                        square.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "▣",
                                        egui::FontId::proportional(26.0),
                                        Color32::from_rgb(
                                            pal.text_weak.0,
                                            pal.text_weak.1,
                                            pal.text_weak.2,
                                        ),
                                    );
                                }
                            }
                            ui.painter().text(
                                egui::pos2(square.center().x, square.bottom() + 4.0),
                                egui::Align2::CENTER_TOP,
                                winrust_core::i18n::end(&app.name, 14),
                                egui::FontId::proportional(10.5),
                                Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
                            );
                            if resp.clicked() {
                                *launch = Some(app.path.clone());
                            }
                            resp.on_hover_text(app.path.display().to_string());
                        }
                    });
                    chunk_start = chunk_end;
                }
            });
    }

    /// The bottom row: Settings, the account, and the power button.
    #[allow(clippy::too_many_arguments)]
    fn start_footer(
        &mut self,
        ui: &mut egui::Ui,
        ask_power: &mut Option<PowerAction>,
        open_settings: &mut bool,
        switch_user: &mut bool,
        switched_window: &mut bool,
    ) {
        let user = winrust_core::system::user_display_name();
        ui.horizontal(|ui| {
            // Left: the account, with the switch-user action in its menu.
            ui.menu_button(RichText::new(format!("👤 {user}")).size(13.0), |ui| {
                ui.set_min_width(220.0);
                if ui.button(t("Access to the machine")).clicked() {
                    let _ = winrust_core::shell::launch(
                        "rundll32.exe",
                        "shell32.dll,Control_RunDLL userpasswords",
                    );
                    ui.close_menu();
                }
                if ui.button(t("Switch user")).clicked() {
                    *switch_user = true;
                    ui.close_menu();
                }
                if ui.button(t("Lock")).clicked() {
                    winrust_core::shell::lock_workstation();
                    ui.close_menu();
                }
            });
            ui.separator();
            if ui.button(format!("⚙ {}", t("Settings"))).clicked() {
                *open_settings = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right: the power button, opening a list of actions that each
                // ask for confirmation.
                ui.menu_button(format!("⏻ {}", t("Power")), |ui| {
                    ui.set_min_width(220.0);
                    for action in PowerAction::ALL {
                        // Hibernation is greyed out where the machine forbids
                        // it, instead of being a button that does nothing.
                        let enabled = action != PowerAction::Hibernate
                            || winrust_core::shell::hibernate_allowed();
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(format!("{}  {}", action.glyph(), action.label())),
                            )
                            .clicked()
                        {
                            *ask_power = Some(action);
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                // The running-window switcher: the only part of the old menu
                // the review did not ask to remove, kept as one compact line.
                let count = self.open_windows.len();
                ui.menu_button(
                    RichText::new(tf!("{} open", count)).size(12.0).weak(),
                    |ui| {
                        ui.set_min_width(260.0);
                        for w in self.open_windows.iter().take(12) {
                            if ui
                                .button(RichText::new(winrust_core::i18n::end(&w.title, 46)).size(12.0))
                                .clicked()
                            {
                                winrust_core::window::activate_window(w.pid);
                                *switched_window = true;
                                ui.close_menu();
                            }
                        }
                    },
                );
            });
        });
    }

    /// The "are you sure?" window for a pending power action.
    ///
    /// It is drawn by the shell's own frame, not inside the Start area, so the
    /// menu can close (it does, when the action is asked for) without taking
    /// the question with it.
    pub fn power_confirmation(&mut self, ctx: &egui::Context) {
        let Some(action) = self.session.pending_power() else {
            return;
        };
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(t("Power"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.add_space(4.0);
                ui.label(RichText::new(action.question()).size(15.0));
                if action == PowerAction::SignOut {
                    ui.label(
                        RichText::new(ts("Unsaved work in open programs will be lost."))
                            .small()
                            .weak(),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(format!("{} {}", action.glyph(), action.label())).clicked() {
                        confirmed = true;
                    }
                    if ui.button(t("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancelled = true;
                    }
                });
            });
        if confirmed {
            self.session.confirm_power();
        } else if cancelled {
            self.session.cancel_power();
        }
    }
}

/// The icon bytes for a Start-menu entry: an `.exe` has its own icon, every
/// other shortcut goes through the shell, which resolves `.lnk` targets.
fn shortcut_icon(path: &std::path::Path) -> Option<Vec<u8>> {
    let is_exe = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);
    if is_exe {
        winrust_core::window::exe_icon_rgba(&path.to_string_lossy(), 48)
    } else {
        winrust_core::shell::file_info_sized(path, 48).icon_rgba
    }
}

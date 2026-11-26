//! The Start menu: a list of every app on the left, pinned tiles on the
//! right, and the account / Settings / power row at the bottom.
//!
//! The layout is Windows 11's, and the split is what the 0.9.1 review asked
//! for: the old menu was a single six-per-row grid of text buttons, which
//! turned "find an application" into reading sixty identical rectangles.  Here
//!
//! * the **left** column lists every installed app once, alphabetically, with
//!   the icon the shell reports for it, and a search box on top;
//! * the **right** column is the pinned grid: square tiles with the icon and
//!   the name under it, which is the part of the menu a user clicks without
//!   reading;
//! * the **bottom** row holds exactly three things - Settings, the current
//!   user, and the power button - and the power button opens a list, not a
//!   menu that fires on hover.  Every power entry asks for a confirmation.
//!
//! ## Why the drawing is split into free functions
//!
//! The two columns and the footer draw from values prepared before the frame
//! (icons as texture ids, because loading one needs `&mut DesktopApp`), and
//! they report the user's intent in a [`MenuOutput`] which the shell applies
//! once the frame is done.  The result is that drawing code cannot perform an
//! action half-way through a frame, and the same functions are usable from any
//! future surface that shows the app list.

use eframe::egui;
use egui::{Color32, RichText};
use std::path::PathBuf;
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::session::{AppEntry, PowerAction};
use crate::DesktopApp;

/// Installed applications: Start Menu shortcuts (`*.lnk`) from both the user
/// and machine-wide folders, plus WinRust's own programs.
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
    // WinRust's own programs.  They are pushed *before* the sort now: the
    // 0.9.0 list prepended them afterwards, so four entries sat out of order
    // at the top of an otherwise alphabetical list.
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

/// The applications the pinned grid shows.
///
/// Windows pins what its vendors install; a bare install has no pin database
/// we can read without COM, so the shell pins its own programs plus the
/// handful of things Windows always ships.  The point of the grid is to be
/// clickable without reading, so only paths that exist are kept.
fn default_pins() -> Vec<AppEntry> {
    let mut pins: Vec<AppEntry> = Vec::new();
    for (name, exe) in [
        ("WinRust Explorer", "winrust-explorer.exe"),
        ("WinRust Task Manager", "winrust-taskmgr.exe"),
        ("WinRust Services", "winrust-services.exe"),
        ("WinRust System Control", "winrust-sysctl.exe"),
        ("WinRust Launcher", "WinRust.exe"),
        ("Notepad", "notepad.exe"),
        ("Paint", "mspaint.exe"),
        ("Calculator", "calc.exe"),
        ("Command Prompt", "cmd.exe"),
        ("Control Panel", "control.exe"),
        ("Task Manager", "taskmgr.exe"),
        ("File Explorer", "explorer.exe"),
    ] {
        if let Some(path) = winrust_core::install::find_binary(exe) {
            pins.push(AppEntry {
                name: t(name),
                path,
            });
        }
    }
    pins
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

/// What one frame of the menu decided; applied by [`DesktopApp`] afterwards.
#[derive(Default)]
struct MenuOutput {
    launch: Option<PathBuf>,
    ask_power: Option<PowerAction>,
    open_settings: bool,
    switch_user: bool,
    switched_window: bool,
}

impl MenuOutput {
    /// Applies the intent recorded while drawing.  The search text is *not*
    /// part of this: it is live state of the text box, and the shell writes it
    /// back before calling here.
    fn apply(self, app: &mut DesktopApp) {
        if let Some(path) = self.launch {
            app.session.close();
            app.open_path(&path);
        }
        if self.open_settings {
            app.session.close();
            // `WinRust.exe --settings`; the launcher is the settings surface.
            if let Some(path) = winrust_core::install::find_binary("WinRust.exe") {
                let _ = winrust_core::shell::execute(&path, "--settings", None);
            }
        }
        if self.switch_user {
            app.session.close();
            let _ = winrust_core::shell::launch("tsdiscon.exe", "");
        }
        if let Some(action) = self.ask_power {
            // Hibernation needs the machine to allow it; say so instead of
            // doing nothing.
            if action == PowerAction::Hibernate && !winrust_core::shell::hibernate_allowed() {
                app.status = ts("Hibernation is not enabled on this PC").to_string();
            } else {
                app.session.ask_power(action);
            }
        }
        if self.switched_window {
            app.session.close();
        }
    }
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
        // screen.  Anchored from the bar's top edge so the bottom row is always
        // visible - the 0.9.0 menu anchored from the screen bottom and its
        // footer fell behind the bar on short displays.
        let width = 620.0_f32.min(screen.width() - 40.0);
        let max_height =
            (screen.bottom() - crate::taskbar::HEIGHT - 16.0 - screen.top()).max(280.0);
        let height = 560.0_f32.min(max_height);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                (screen.center().x - width / 2.0).max(screen.left() + 8.0),
                screen.bottom() - crate::taskbar::HEIGHT - 8.0 - height,
            ),
            egui::vec2(width, height),
        );

        // Icons are loaded here, before the drawing: `file_icon_texture` needs
        // `&mut self`, and doing it mid-frame would borrow the shell twice.
        let apps = self.session.apps().to_vec();
        let pins = default_pins();
        let apps: Vec<(AppEntry, Option<egui::TextureId>)> = apps
            .into_iter()
            .map(|a| {
                let slot = format!("start_icon|{}", a.path.display());
                let path = a.path.clone();
                let id = self.file_icon_texture(ctx, slot, || shortcut_icon(&path));
                (a, id)
            })
            .collect();
        let pins: Vec<(AppEntry, Option<egui::TextureId>)> = pins
            .into_iter()
            .map(|a| {
                let slot = format!("pin_icon|{}", a.path.display());
                let path = a.path.clone();
                let id = self.file_icon_texture(ctx, slot, || shortcut_icon(&path));
                (a, id)
            })
            .collect();
        let pal = winrust_ui::theme::pal(&self.appearance);
        let open_windows = self.open_windows.clone();
        let user = winrust_core::system::user_display_name();
        // Rise 10 px and fade in, like the tray flyouts.
        let slide = (1.0 - anim) * 10.0;
        let alpha = (anim * 250.0) as u8;
        let mut output = MenuOutput::default();
        // The search text is a local copy for the frame.  `TextEdit` needs
        // `&mut String` for the whole frame, and the session must stay
        // borrowable after the drawing closures end, so the two accessors
        // cannot overlap.  A copy also keeps the filter and the field in sync
        // when the box is edited.
        let mut search = self.session.search().to_string();

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
                        ui.add_sized(
                            [ui.available_width(), 30.0],
                            egui::TextEdit::singleline(&mut search)
                                .hint_text(format!("🔍 {}", t("Search apps"))),
                        );
                        ui.add_space(8.0);
                        // The footer row is reserved first, so the columns get
                        // whatever is left and can never push it out of view.
                        let body_h = (ui.available_height() - 42.0).max(120.0);
                        let needle = search.to_lowercase();
                        ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                            ui.columns(2, |cols| {
                                let (list, pinned) = cols.split_at_mut(1);
                                app_list(&mut list[0], &apps, &needle, pal, &mut output);
                                pinned_grid(&mut pinned[0], &pins, pal, &mut output);
                            });
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        footer(ui, &open_windows, &user, &mut output);
                    });
            })
            .response;

        // The search box may have been edited this frame; the change goes into
        // the session first, then the rest of the intent is applied.
        self.session.search_mut().clear();
        self.session.search_mut().push_str(&search);
        output.apply(self);

        // Clicking outside closes the menu.
        if self.session.is_open() && ctx.input(|i| i.pointer.any_click()) {
            if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
                if !response.rect.contains(p) {
                    self.session.close();
                }
            }
        }
    }

    /// The "are you sure?" window for a pending power action.
    ///
    /// Drawn by the shell's own frame, not inside the Start area, so the menu
    /// can close (it does, when the action is asked for) without taking the
    /// question with it.
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
                    if ui
                        .button(format!("{} {}", action.glyph(), action.label()))
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button(t("Cancel")).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
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

/// The left column: every app, alphabetical, icon + name, one per row.
fn app_list(
    ui: &mut egui::Ui,
    apps: &[(AppEntry, Option<egui::TextureId>)],
    needle: &str,
    pal: winrust_ui::theme::Palette,
    out: &mut MenuOutput,
) {
    egui::ScrollArea::vertical()
        .id_salt("start_app_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut shown = 0usize;
            for (app, icon) in apps {
                if !needle.is_empty() && !app.name.to_lowercase().contains(needle) {
                    continue;
                }
                shown += 1;
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        5.0,
                        Color32::from_rgba_unmultiplied(pal.hover.0, pal.hover.1, pal.hover.2, 90),
                    );
                }
                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 18.0, rect.center().y),
                    egui::vec2(18.0, 18.0),
                );
                match icon {
                    Some(id) => {
                        ui.painter().image(
                            *id,
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
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
                    out.launch = Some(app.path.clone());
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
fn pinned_grid(
    ui: &mut egui::Ui,
    pins: &[(AppEntry, Option<egui::TextureId>)],
    pal: winrust_ui::theme::Palette,
    out: &mut MenuOutput,
) {
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
            let mut start = 0usize;
            while start < pins.len() {
                let end = (start + per_row).min(pins.len());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (app, icon) in &pins[start..end] {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(tile, tile + 18.0),
                            egui::Sense::click(),
                        );
                        let square = egui::Rect::from_min_size(rect.min, egui::vec2(tile, tile));
                        let fill = if resp.hovered() {
                            Color32::from_rgba_unmultiplied(
                                pal.hover.0,
                                pal.hover.1,
                                pal.hover.2,
                                120,
                            )
                        } else {
                            Color32::from_rgba_unmultiplied(pal.tile.0, pal.tile.1, pal.tile.2, 255)
                        };
                        ui.painter().rect_filled(square, 8.0, fill);
                        let icon_rect =
                            egui::Rect::from_center_size(square.center(), egui::vec2(34.0, 34.0));
                        match icon {
                            Some(id) => {
                                ui.painter().image(
                                    *id,
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
                            out.launch = Some(app.path.clone());
                        }
                        resp.on_hover_text(app.path.display().to_string());
                    }
                });
                start = end;
            }
        });
}

/// The bottom row: the account, Settings, the running-window list and power.
fn footer(
    ui: &mut egui::Ui,
    open_windows: &[crate::taskbar::WindowEntry],
    user: &str,
    out: &mut MenuOutput,
) {
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
                out.switch_user = true;
                ui.close_menu();
            }
            if ui.button(t("Lock")).clicked() {
                winrust_core::shell::lock_workstation();
                ui.close_menu();
            }
        });
        ui.separator();
        if ui.button(format!("⚙ {}", t("Settings"))).clicked() {
            out.open_settings = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right: the power button, opening a list whose every entry asks
            // for confirmation.
            ui.menu_button(format!("⏻ {}", t("Power")), |ui| {
                ui.set_min_width(220.0);
                for action in PowerAction::ALL {
                    // Hibernation is greyed out where the machine forbids it,
                    // instead of being a button that does nothing.
                    let enabled = action != PowerAction::Hibernate
                        || winrust_core::shell::hibernate_allowed();
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(format!("{}  {}", action.glyph(), action.label())),
                        )
                        .clicked()
                    {
                        out.ask_power = Some(action);
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            // The running-window switcher: the one part of the old menu the
            // review did not ask to remove, kept as a single compact line.
            let count = open_windows.len();
            ui.menu_button(
                RichText::new(tf!("{} open", count)).size(12.0).weak(),
                |ui| {
                    ui.set_min_width(260.0);
                    for w in open_windows.iter().take(12) {
                        if ui
                            .button(
                                RichText::new(winrust_core::i18n::end(&w.title, 46)).size(12.0),
                            )
                            .clicked()
                        {
                            winrust_core::window::activate_window(w.pid);
                            out.switched_window = true;
                            ui.close_menu();
                        }
                    }
                },
            );
        });
    });
}

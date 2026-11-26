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

use crate::model::{AppEntry, PowerAction};
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
    /// Where the power button was drawn, so the list can hang above the bar.
    power_menu_button: Option<egui::Rect>,
    /// The pin whose menu asked to open its folder / properties, or to be
    /// removed.  Applied after the menu closes, when `&mut self` is free.
    reveal: Option<PathBuf>,
    props: Option<PathBuf>,
    unpin: Option<PathBuf>,
    /// The two nested lists: whether the frame opened or closed one.
    toggle_power_menu: bool,
    toggle_user_menu: bool,
    /// Set when an entry of either list was picked, so the list closes.
    close_power_menu: bool,
    close_user_menu: bool,
    /// An app-list entry asking to be pinned.
    pin: Option<PathBuf>,
    /// Pin reordering, completed: the tile that was dragged and the slot it
    /// was dropped on.  The in-flight state lives in egui's memory, because a
    /// drag spans frames and `MenuOutput` is rebuilt from scratch each one.
    pin_move: Option<(usize, usize)>,
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
            // The Windows settings app, exactly as `Win+I` opens it.  This used
            // to launch `WinRust.exe --settings`, i.e. the *launcher's* own
            // options, which is not what "Settings" in a Start menu means.
            let _ = winrust_core::shell::open_shell_path("ms-settings:");
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
        if let Some(path) = self.reveal {
            if let Some(dir) = path.parent() {
                let _ = winrust_core::shell::open(dir);
            }
        }
        if let Some(path) = self.props {
            let _ = winrust_core::shell::show_properties(&path);
        }
        if let Some(path) = self.unpin {
            app.session.unpin(&path);
        }
        if let Some(path) = self.pin {
            app.session.pin(&path);
        }
        if self.toggle_power_menu {
            app.session.toggle_power_menu();
        }
        if self.toggle_user_menu {
            app.session.toggle_user_menu();
        }
        if self.close_power_menu {
            app.session.close_power_menu();
        }
        if self.close_user_menu {
            app.session.close_user_menu();
        }
        // A completed pin drag: move the dragged entry to where it was dropped.
        if let Some((from, to)) = self.pin_move {
            if from != to {
                app.session.move_pin(from, to);
            }
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
        // Pinned to the left edge, above the Start button, the way the Windows
        // 10 menu is - it used to be centred on the screen.
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                screen.left() + 8.0,
                screen.bottom() - crate::taskbar::HEIGHT - 8.0 - height,
            ),
            egui::vec2(width, height),
        );

        // Icons are loaded here, before the drawing: `file_icon_texture` needs
        // `&mut self`, and doing it mid-frame would borrow the shell twice.
        let apps = self.session.apps().to_vec();
        // Pinned entries come from the session, so the user's set survives a
        // restart.  The defaults are only used until the user edits them.
        let pins = self.session.pins();
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
        // The two nested lists live in the session across frames; the menu's
        // output only carries the click that changes them.
        let app_session_power_open = self.session.power_menu_open();
        let app_session_user_open = self.session.user_menu_open();
        // Rise 10 px and fade in, like the tray flyouts.
        let slide = (1.0 - anim) * 10.0;
        let alpha = (anim * 250.0) as u8;
        let mut output = MenuOutput {
            // The power list's state lives in the session across frames; the
            // struct is rebuilt every frame.
            ..MenuOutput::default()
        };
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
                        footer(
                            ui,
                            &open_windows,
                            &user,
                            pal,
                            app_session_power_open,
                            app_session_user_open,
                            &mut output,
                        );
                    });
            })
            .response;

        // The search box may have been edited this frame; the change goes into
        // the session first, then the rest of the intent is applied.
        self.session.search_mut().clear();
        self.session.search_mut().push_str(&search);
        output.apply(self);

        // Clicking outside closes the menu - except on the frame it was
        // opened.  The Start button toggles the menu in the taskbar pass,
        // which runs *before* this function, so the click that opened it is
        // still "a click outside the menu" here and closed it again in the
        // same frame: the menu flashed and disappeared.
        if self.session.is_open()
            && !self.session.opened_this_frame(self.frame_no)
            && ctx.input(|i| i.pointer.any_click())
        {
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
                // The other half of "add and remove pins": any app in the full
                // list can be pinned from its own menu.
                resp.context_menu(|ui| {
                    ui.set_min_width(210.0);
                    if ui.button(t("Open")).clicked() {
                        out.launch = Some(app.path.clone());
                        ui.close_menu();
                    }
                    if ui.button(t("File location")).clicked() {
                        out.reveal = Some(app.path.clone());
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(t("Pin to Start")).clicked() {
                        out.pin = Some(app.path.clone());
                        ui.close_menu();
                    }
                    if ui.button(t("Properties")).clicked() {
                        out.props = Some(app.path.clone());
                        ui.close_menu();
                    }
                });
                resp.on_hover_text(app.path.display().to_string());
            }
            if shown == 0 {
                ui.add_space(8.0);
                ui.label(RichText::new(ts("Nothing found")).weak().small());
            }
        });
}

/// The right column: pinned apps as square icon tiles with the name below.
///
/// The grid is *free-form*, the way Windows 11's pinned area behaves: empty
/// slots are drawn as dimmed placeholders and a tile may be dropped on any of
/// them, not only on a tile that is already occupied.  Reordering compacts
/// afterwards, so the dropped tile really travels to the slot the user aimed
/// at instead of snapping back to the end of the list.
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
    let per_row = ((ui.available_width() + gap) / (tile + gap))
        .floor()
        .max(1.0) as usize;
    // One spare row of empty slots: enough to drop "past the end", and it is
    // what makes the area read as a grid rather than a ragged list.
    let rows = pins.len().div_ceil(per_row).max(1) + 1;
    let slots = rows * per_row;

    let drag_id = egui::Id::new("start_pin_drag");
    let mut drop_slot: Option<usize> = None;

    egui::ScrollArea::vertical()
        .id_salt("start_pins")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut slot = 0usize;
            while slot < slots {
                let end = (slot + per_row).min(slots);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for index in slot..end {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(tile, tile + 18.0),
                            egui::Sense::click_and_drag(),
                        );
                        let square = egui::Rect::from_min_size(rect.min, egui::vec2(tile, tile));
                        let pin = pins.get(index);
                        let dragged: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));
                        if resp.drag_started() {
                            if let Some((_, _)) = pin {
                                ui.ctx().data_mut(|d| d.insert_temp(drag_id, index));
                            }
                        }
                        // Target highlight: the slot under the pointer while a
                        // drag is live, whether or not it already holds a tile.
                        let is_target = resp.hovered() && dragged.is_some_and(|from| from != index);
                        match pin {
                            Some((app, icon)) => {
                                let fill = if resp.hovered() && dragged.is_none() {
                                    Color32::from_rgba_unmultiplied(
                                        pal.hover.0,
                                        pal.hover.1,
                                        pal.hover.2,
                                        120,
                                    )
                                } else {
                                    Color32::from_rgba_unmultiplied(
                                        pal.tile.0, pal.tile.1, pal.tile.2, 255,
                                    )
                                };
                                // The tile being dragged is drawn faint, which
                                // is the ghost: it stays where it was so the
                                // pointer can be followed with the eye.
                                let is_source = dragged == Some(index);
                                ui.painter().rect_filled(
                                    square,
                                    8.0,
                                    if is_source {
                                        Color32::from_rgba_unmultiplied(
                                            pal.tile.0, pal.tile.1, pal.tile.2, 90,
                                        )
                                    } else {
                                        fill
                                    },
                                );
                                let icon_rect = egui::Rect::from_center_size(
                                    square.center(),
                                    egui::vec2(34.0, 34.0),
                                );
                                match icon {
                                    Some(id) => {
                                        ui.painter().image(
                                            *id,
                                            icon_rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            if is_source {
                                                Color32::from_white_alpha(110)
                                            } else {
                                                Color32::WHITE
                                            },
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
                                // The pin's own context menu, in the order the
                                // review asked for: Open, file location,
                                // Unpin, Properties.  "Pin to Start" lives on
                                // the app list's menu, so the set is editable
                                // from both sides.
                                resp.context_menu(|ui| {
                                    ui.set_min_width(210.0);
                                    if ui.button(t("Open")).clicked() {
                                        out.launch = Some(app.path.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button(t("File location")).clicked() {
                                        out.reveal = Some(app.path.clone());
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button(t("Unpin from Start")).clicked() {
                                        out.unpin = Some(app.path.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button(t("Properties")).clicked() {
                                        out.props = Some(app.path.clone());
                                        ui.close_menu();
                                    }
                                });
                                resp.on_hover_text(app.path.display().to_string());
                            }
                            None => {
                                // An empty slot: a dimmed square.  It is a drop
                                // target like any other, which is what makes the
                                // grid free-form.
                                ui.painter().rect_filled(
                                    square,
                                    8.0,
                                    Color32::from_rgba_unmultiplied(
                                        pal.tile.0,
                                        pal.tile.1,
                                        pal.tile.2,
                                        if resp.hovered() { 110 } else { 60 },
                                    ),
                                );
                                ui.painter().rect_stroke(
                                    square,
                                    8.0,
                                    egui::Stroke::new(
                                        1.0_f32,
                                        Color32::from_rgba_unmultiplied(
                                            pal.line.0, pal.line.1, pal.line.2, 120,
                                        ),
                                    ),
                                );
                            }
                        }
                        if is_target {
                            ui.painter().rect_stroke(
                                square.shrink(1.0),
                                8.0,
                                egui::Stroke::new(2.0_f32, Color32::from_rgb(0x60, 0xa8, 0xff)),
                            );
                        }
                        if is_target && ui.ctx().input(|i| i.pointer.primary_released()) {
                            drop_slot = Some(index);
                        }
                    }
                });
                slot = end;
            }
        });

    if let (Some(from), Some(to)) = (ui.ctx().data(|d| d.get_temp::<usize>(drag_id)), drop_slot) {
        if from != to {
            out.pin_move = Some((from, to));
        }
    }
    // The gesture is over: drop the remembered tile *after* every slot had a
    // chance to read it, otherwise the first slot would clear the state before
    // the slot under the pointer could commit the drop.
    if ui.ctx().input(|i| i.pointer.primary_released()) {
        ui.ctx().data_mut(|d| d.remove_temp::<usize>(drag_id));
    }
}

/// The surface the two footer lists are drawn on, matching the tray's flyouts.
fn popup_surface(pal: winrust_ui::theme::Palette) -> Color32 {
    Color32::from_rgba_unmultiplied(pal.surface.0, pal.surface.1, pal.surface.2, 252)
}

/// The outline of the two footer lists, matching the tray's flyouts.
fn popup_line(pal: winrust_ui::theme::Palette) -> Color32 {
    Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2)
}

/// The bottom row: the account, Settings, the running-window list and power.
fn footer(
    ui: &mut egui::Ui,
    open_windows: &[crate::taskbar::WindowEntry],
    user: &str,
    pal: winrust_ui::theme::Palette,
    // Whether the session already has each nested list open.  Read in `draw`
    // and passed in, because the two lists are drawn here in the footer.
    power_open: bool,
    user_open: bool,
    out: &mut MenuOutput,
) {
    ui.horizontal(|ui| {
        // Left: the account.  Its list is an area pinned above the taskbar, for
        // the same reason as the power list: an egui submenu follows the cursor
        // and hung off the bottom of the screen.
        let user_resp = ui.button(RichText::new(format!("👤 {user}")).size(13.0));
        if user_resp.clicked() {
            out.toggle_user_menu = true;
        }
        if user_open {
            let screen = ui.ctx().screen_rect();
            winrust_ui::anchored_above(
                ui.ctx(),
                egui::Id::new("start_user_menu"),
                user_resp.rect,
                screen.bottom() - crate::taskbar::HEIGHT,
                6.0,
                |ui| {
                    // Same frame as the tray's flyouts.  Without it the Area
                    // painted its buttons straight onto whatever was behind
                    // them, so the list had no surface and no outline - the
                    // "списки висят сами по себе без общей обводки" report.
                    egui::Frame::popup(ui.style())
                        .fill(popup_surface(pal))
                        .stroke(egui::Stroke::new(1.0_f32, popup_line(pal)))
                        .rounding(10.0)
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.set_min_width(200.0);
                            if ui.button(t("Access to the machine")).clicked() {
                                let _ = winrust_core::shell::launch(
                                    "rundll32.exe",
                                    "shell32.dll,Control_RunDLL userpasswords",
                                );
                            }
                            if ui.button(t("Switch user")).clicked() {
                                out.switch_user = true;
                                out.close_user_menu = true;
                            }
                            if ui.button(t("Lock")).clicked() {
                                winrust_core::shell::lock_workstation();
                                out.close_user_menu = true;
                            }
                        });
                },
            );
        }
        ui.separator();
        if ui.button(format!("⚙ {}", t("Settings"))).clicked() {
            out.open_settings = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right: the power button, opening a list whose every entry asks
            // for confirmation.
            let power_resp = ui.button(format!("⏻ {}", t("Power")));
            if power_resp.clicked() {
                out.toggle_power_menu = true;
            }
            out.power_menu_button = Some(power_resp.rect);
            if power_open {
                // The list is its own area pinned above the *screen's* bar, not
                // an egui submenu: a submenu is positioned by the cursor and
                // ran off the bottom of the screen - "меню завершения работы
                // съезжает вниз".  `anchored_above` puts its bottom edge on the
                // taskbar's top edge, the way Windows does.
                let button = power_resp.rect;
                let screen = ui.ctx().screen_rect();
                winrust_ui::anchored_above(
                    ui.ctx(),
                    egui::Id::new("start_power_menu"),
                    button,
                    screen.bottom() - crate::taskbar::HEIGHT,
                    6.0,
                    |ui| {
                        egui::Frame::popup(ui.style())
                            .fill(popup_surface(pal))
                            .stroke(egui::Stroke::new(1.0_f32, popup_line(pal)))
                            .rounding(10.0)
                            .inner_margin(egui::Margin::same(10.0))
                            .show(ui, |ui| {
                                ui.set_min_width(200.0);
                                for action in PowerAction::ALL {
                                    let enabled = action != PowerAction::Hibernate
                                        || winrust_core::shell::hibernate_allowed();
                                    if ui
                                        .add_enabled(
                                            enabled,
                                            egui::Button::new(format!(
                                                "{}  {}",
                                                action.glyph(),
                                                action.label()
                                            )),
                                        )
                                        .clicked()
                                    {
                                        out.ask_power = Some(action);
                                        out.close_power_menu = true;
                                    }
                                }
                            });
                    },
                );
            }
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
                            .button(RichText::new(winrust_core::i18n::end(&w.title, 46)).size(12.0))
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

//! Desktop icons: the user's Desktop folder, drawn as a grid.
//!
//! Windows' own desktop is a list view over `%USERPROFILE%\Desktop` plus the
//! two virtual entries (This PC and the Recycle Bin).  We do the same, but the
//! grid position is persisted in `%APPDATA%\WinRust\desktop.json`, so icons stay
//! where the user puts them across sign-ins - the shell must not shuffle the
//! desktop every time it starts.
//!
//! Two things a desktop lives or dies by are here as well: the icon pictures
//! come from the shell (`SHGetFileInfoW`, so a `.lnk` shows the icon of what it
//! points at), and dragging paints a translucent ghost under the cursor while
//! the target cell is outlined, exactly like Explorer does.

use eframe::egui;
use egui::Color32;
use std::collections::HashMap;
use std::path::PathBuf;
use winrust_core::i18n::ts;
use winrust_core::shell::KnownFolder;
use winrust_core::{t, tf};

use crate::DesktopApp;

#[derive(Debug, Clone)]
pub struct DesktopIcon {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Grid position `(column, row)`.
    pub cell: (usize, usize),
    /// Free position in *cells*, used when "Align icons to grid" is off.
    ///
    /// With the grid on, an icon lives on `cell`; with it off, it lives exactly
    /// where it was dropped, and `free` records that as fractional cell
    /// coordinates so it survives a restart and a view-size change in the same
    /// units as the grid.  `None` means "never placed by hand".
    pub free: Option<(f32, f32)>,
    /// `shell:` namespace path for a virtual entry (Recycle Bin, This PC).
    /// `path` alone cannot express them: the Recycle Bin's filesystem path is
    /// `C:\$Recycle.Bin\<SID>`, which is not the same thing the user sees.
    pub shell_path: Option<&'static str>,
}

/// Default grid cell.  `spacing` in the settings scales it.
pub const CELL: egui::Vec2 = egui::vec2(96.0, 92.0);
/// Icon sizes offered by the View submenu.
pub const ICON_SIZES: [f32; 3] = [48.0, 32.0, 24.0];
/// Default icon picture size in pixels.
pub const DEFAULT_ICON: f32 = 48.0;

/// Which sort order the desktop uses (View -> Sort by).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Name,
    Size,
    Type,
    Modified,
}

impl SortOrder {
    pub const ALL: [SortOrder; 4] = [
        SortOrder::Name,
        SortOrder::Size,
        SortOrder::Type,
        SortOrder::Modified,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Name => "Name",
            SortOrder::Size => "Size",
            SortOrder::Type => "Type",
            SortOrder::Modified => "Date modified",
        }
    }
}

/// Desktop-only switches (View menu).  Stored as `settings.json` next to the
/// icon positions, so they survive the shell restarting at sign-in.
#[derive(Debug, Clone, Copy)]
pub struct DesktopSettings {
    /// Cell spacing as a percentage of the default (50..=200).
    pub spacing: u8,
    /// Snap icons to the grid when dragged.
    pub snap_to_grid: bool,
    /// Draw the icons at all (View -> Show desktop icons).
    pub show_icons: bool,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            spacing: 100,
            snap_to_grid: true,
            show_icons: true,
        }
    }
}

impl DesktopSettings {
    fn path() -> Option<PathBuf> {
        positions_path().map(|p| p.with_file_name("desktop_settings.json"))
    }

    pub fn load() -> Self {
        let d = Self::default();
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(|v: serde_json::Value| Self {
                spacing: v
                    .get("spacing")
                    .and_then(|x| x.as_u64())
                    .map(|x| x.clamp(50, 200) as u8)
                    .unwrap_or(d.spacing),
                snap_to_grid: v
                    .get("snap_to_grid")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(d.snap_to_grid),
                show_icons: v
                    .get("show_icons")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(d.show_icons),
            })
            .unwrap_or(d)
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let v = serde_json::json!({
            "spacing": self.spacing,
            "snap_to_grid": self.snap_to_grid,
            "show_icons": self.show_icons,
        });
        if let Ok(text) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(path, text);
        }
    }
}

fn positions_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(base).join("WinRust");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("desktop.json"))
}

/// One icon's saved place: its grid cell, and the exact place it was dropped
/// when the grid was off.
///
/// The file used to be `{name: [col, row]}`.  That cannot express "this icon is
/// where the user put it, not on the lattice", so switching the grid off left
/// nothing to restore and the icon jumped back.  `free` is optional and older
/// files still load, because a missing field defaults to `None`.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct SavedPos {
    cell: (usize, usize),
    #[serde(default)]
    free: Option<(f32, f32)>,
}

/// Positions as read from disk.  Accepts both the old array form and the new
/// object form, so an upgrade does not lose the layout.
fn load_positions() -> HashMap<String, SavedPos> {
    let Some(text) = positions_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return HashMap::new();
    };
    let Ok(raw) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for (name, value) in raw {
        // New form.
        if let Ok(pos) = serde_json::from_value::<SavedPos>(value.clone()) {
            out.insert(name, pos);
            continue;
        }
        // Old form: a two-element array.
        if let Ok(cell) = serde_json::from_value::<(usize, usize)>(value) {
            out.insert(name, SavedPos { cell, free: None });
        }
    }
    out
}

pub fn save_positions(icons: &[DesktopIcon]) {
    let Some(path) = positions_path() else {
        return;
    };
    let map: HashMap<&str, SavedPos> = icons
        .iter()
        .map(|i| {
            (
                i.name.as_str(),
                SavedPos {
                    cell: i.cell,
                    free: i.free,
                },
            )
        })
        .collect();
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, text);
    }
}

/// Enumerate the desktop: two virtual entries first, then the folder contents.
pub fn scan(order: SortOrder, descending: bool) -> Vec<DesktopIcon> {
    let mut out: Vec<DesktopIcon> = Vec::new();
    let positions = load_positions();

    out.push(DesktopIcon {
        name: ts("This PC").to_string(),
        path: KnownFolder::Computer
            .path()
            .unwrap_or_else(|| PathBuf::from("C:\\")),
        is_dir: true,
        cell: (0, 0),
        free: None,
        shell_path: winrust_core::shell::shell_path_of(KnownFolder::Computer),
    });
    out.push(DesktopIcon {
        name: ts("Recycle Bin").to_string(),
        path: KnownFolder::RecycleBin
            .path()
            .unwrap_or_else(|| PathBuf::from("C:\\")),
        is_dir: true,
        cell: (0, 1),
        free: None,
        shell_path: winrust_core::shell::shell_path_of(KnownFolder::RecycleBin),
    });

    let mut real: Vec<DesktopIcon> = Vec::new();
    if let Some(desktop) = KnownFolder::Desktop.path() {
        if let Ok(read) = std::fs::read_dir(&desktop) {
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let is_dir = e.metadata().map(|m| m.is_dir()).unwrap_or(false);
                real.push(DesktopIcon {
                    path: e.path(),
                    name,
                    is_dir,
                    cell: (0, 0),
                    free: None,
                    shell_path: None,
                });
            }
        }
    }

    sort_icons(&mut real, order);
    if descending {
        real.reverse();
    }
    out.extend(real);

    // Restore saved positions, then fill the remaining cells column by column.
    let mut taken: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for (idx, icon) in out.iter_mut().enumerate() {
        if let Some(pos) = positions.get(&icon.name) {
            if taken.insert(pos.cell) {
                icon.cell = pos.cell;
                icon.free = pos.free;
                continue;
            }
        }
        // Keep the two virtual entries pinned if nothing was saved for them.
        if idx < 2 && taken.insert(icon.cell) {
            continue;
        }
        let mut slot = 0usize;
        loop {
            let cell = (slot / 8, slot % 8);
            slot += 1;
            if taken.insert(cell) {
                icon.cell = cell;
                break;
            }
        }
    }
    out
}

/// Sort the real entries the way View -> Sort by asks for.
pub fn sort_icons(icons: &mut [DesktopIcon], order: SortOrder) {
    match order {
        SortOrder::Name => icons.sort_by_key(|i| i.name.to_lowercase()),
        SortOrder::Size => {
            icons.sort_by_key(|i| std::fs::metadata(&i.path).map(|m| m.len()).unwrap_or(0))
        }
        SortOrder::Type => icons.sort_by_key(|i| {
            let ext = i
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            (ext, i.name.to_lowercase())
        }),
        SortOrder::Modified => icons.sort_by_key(|i| {
            std::fs::metadata(&i.path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }),
    }
}

/// Everything the desktop asked for this frame, applied once the loop is done so
/// no borrow of `self` is alive while the filesystem is touched.
#[derive(Default)]
struct FrameActions {
    open: Option<DesktopIcon>,
    moved: Option<(String, (usize, usize), Option<(f32, f32)>)>,
    /// "Align icons to grid" was switched back on: tidy what is scattered.
    snap_now: bool,
    clicked: Option<(usize, bool, bool)>,
    new_folder: bool,
    /// New -> <kind>: an empty file with that extension on the Desktop.  A
    /// `String` because the template list comes from the registry at run time,
    /// not from a fixed set of extensions.
    new_file: Option<String>,
    refresh: bool,
    sort: Option<SortOrder>,
    order_toggled: bool,
    size: Option<f32>,
}

impl DesktopApp {
    /// Pixel size of one grid cell for the current icon size.
    fn cell_size(&self) -> egui::Vec2 {
        let base = CELL * (self.settings.spacing.clamp(50, 200) as f32 / 100.0);
        egui::vec2(
            base.x.max(self.icon_size + 16.0),
            base.y.max(self.icon_size + 30.0),
        )
    }

    /// The wallpaper grid.  Double-click opens, drag moves and snaps.
    pub fn desktop_area(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let area = egui::Rect::from_min_max(
            screen.left_top() + egui::vec2(8.0, 8.0),
            egui::pos2(
                screen.right() - 8.0,
                screen.bottom() - crate::taskbar::HEIGHT - 4.0,
            ),
        );

        // One top-level Ui on the Background layer owns every wallpaper widget.
        // egui 0.29 has no `Context::interact`, so the hit areas are registered
        // through a Ui whose clip rect is the whole wallpaper.
        let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("desktop_layer"));
        let ui = egui::Ui::new(
            ctx.clone(),
            layer,
            egui::Id::new("desktop_ui"),
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );

        let mut act = FrameActions::default();
        let mut drop_off_cell: Option<(usize, usize)> = None;
        // The "New" submenu's real contents, read once per frame: the shell's
        // `ShellNew` registrations for this machine, both hives.
        // Icons are loaded before the menu is drawn: the menu closure borrows
        // `self` mutably for the whole `context_menu`, so
        // `file_icon_texture` cannot be called inside it.
        let new_items: Vec<(winrust_core::fileassoc::NewItem, Option<egui::TextureId>)> =
            winrust_core::fileassoc::new_items()
                .into_iter()
                .map(|item| {
                    let slot = format!("new|{}", item.extension);
                    let probe = std::path::PathBuf::from(format!("new.{}", item.extension));
                    let id = self.file_icon_texture(ctx, slot, || {
                        winrust_core::shell::file_info_sized(&probe, 16).icon_rgba
                    });
                    (item, id)
                })
                .collect();
        let draw_icons = self.settings.show_icons;
        let cell = self.cell_size();
        let pal = winrust_ui::theme::pal(&self.appearance);

        // The wallpaper hit area is registered before the icons, because inside a
        // single layer the widget registered last wins the hit test - icons must
        // stay clickable.
        let bg = ui.interact(
            area,
            egui::Id::new("desktop_bg"),
            egui::Sense::click_and_drag(),
        );

        // View / Sort / New folder / Properties on the empty wallpaper.
        bg.context_menu(|ui| {
            ui.menu_button(t("View"), |ui| {
                for size in ICON_SIZES {
                    let label = match size as i32 {
                        48 => t("Large icons"),
                        32 => t("Medium icons"),
                        _ => t("Small icons"),
                    };
                    let selected = (self.icon_size - size).abs() < 1.0;
                    if ui.selectable_label(selected, label).clicked() {
                        act.size = Some(size);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui
                    .selectable_label(self.settings.show_icons, t("Show desktop icons"))
                    .clicked()
                {
                    self.settings.show_icons = !self.settings.show_icons;
                    self.settings_dirty = true;
                    ui.close_menu();
                }
            });
            ui.menu_button(t("Sort by"), |ui| {
                for o in SortOrder::ALL {
                    if ui
                        .selectable_label(self.sort_order == o, t(o.label()))
                        .clicked()
                    {
                        act.sort = Some(o);
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button(t("Ascending")).clicked() {
                    self.sort_desc = false;
                    act.order_toggled = true;
                    ui.close_menu();
                }
                if ui.button(t("Descending")).clicked() {
                    self.sort_desc = true;
                    act.order_toggled = true;
                    ui.close_menu();
                }
                ui.separator();
                // "Align icons to grid" belongs with the *arrangement* of the
                // icons, next to the sort order, and it has to be a toggle:
                // switched off, an icon stays exactly where it is dropped
                // instead of snapping to the nearest cell.
                let mut snap = self.settings.snap_to_grid;
                if ui.checkbox(&mut snap, t("Align icons to grid")).changed() {
                    self.settings.snap_to_grid = snap;
                    if snap {
                        // Turning it back on tidies what is already scattered,
                        // which is what the user expects from a grid.
                        act.snap_now = true;
                    }
                    self.settings_dirty = true;
                }
            });
            if ui.button(t("Refresh")).clicked() {
                act.refresh = true;
                ui.close_menu();
            }
            ui.separator();
            ui.menu_button(t("New"), |ui| {
                ui.set_min_width(230.0);
                if ui.button(t("Folder")).clicked() {
                    act.new_folder = true;
                    ui.close_menu();
                }
                if ui.button(t("Shortcut")).clicked() {
                    let _ =
                        winrust_core::shell::launch("rundll32.exe", "appwiz.cpl,NewLinkHere %1");
                    ui.close_menu();
                }
                ui.separator();
                // The file templates are the ones *this machine* registered
                // under `ShellNew`, not a hard-coded list: a hard-coded one
                // showed Word, Excel, PowerPoint, ZIP and WinRAR on machines
                // that have none of them and hid everything that was really
                // installed.  Each entry carries its program's own icon, read
                // through the association of its extension.
                for (item, icon) in &new_items {
                    let resp = match icon {
                        Some(id) => ui.add(
                            egui::Button::image_and_text(
                                egui::Image::new(egui::load::SizedTexture::new(
                                    *id,
                                    egui::vec2(16.0, 16.0),
                                )),
                                &item.label,
                            )
                            .frame(false),
                        ),
                        None => ui.add(egui::Button::new(&item.label).frame(false)),
                    };
                    if resp.clicked() {
                        act.new_file = Some(item.extension.clone());
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            if ui.button(t("Display settings")).clicked() {
                let _ = winrust_core::shell::launch(
                    "rundll32.exe",
                    "shell32.dll,Control_RunDLL desk.cpl",
                );
                ui.close_menu();
            }
            if ui.button(t("Personalize")).clicked() {
                // "Персонализация" means the Settings page, not the *display*
                // control panel: `desk.cpl,,0` opens background settings for a
                // moment and is the wrong app entirely on Windows 10/11.
                let _ = winrust_core::shell::open_shell_path("ms-settings:personalization");
                ui.close_menu();
            }
        });

        let icons = if draw_icons {
            self.icons.clone()
        } else {
            Vec::new()
        };
        // Selection is a set now: Ctrl+click toggles, a rubber band replaces it.
        let selected_set = self.selection.clone();
        let icon_size = self.icon_size;
        let (pointer, drag_stopped, drag_delta) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.any_released(),
                i.pointer.delta(),
            )
        });

        for (idx, icon) in icons.iter().enumerate() {
            // With the grid on, the icon sits on its cell; with it off, an icon
            // the user has placed sits exactly where it was dropped.  `free`
            // holds fractional cell coordinates, so both survive a restart.
            let (gx, gy) = match (self.settings.snap_to_grid, icon.free) {
                (true, _) => (icon.cell.0 as f32, icon.cell.1 as f32),
                (false, Some(free)) => free,
                (false, None) => (icon.cell.0 as f32, icon.cell.1 as f32),
            };
            let pos = area.left_top() + egui::vec2(gx * cell.x, gy * cell.y);
            let rect = egui::Rect::from_min_size(pos, egui::vec2(cell.x - 6.0, cell.y - 8.0));
            let resp = ui.interact(
                rect,
                egui::Id::new(("desktop_icon", idx)),
                egui::Sense::click_and_drag(),
            );
            let selected = selected_set.contains(&idx);
            let painter = ctx.layer_painter(layer);
            if resp.hovered() || selected {
                let fill = if selected {
                    Color32::from_rgba_unmultiplied(
                        pal.selection.0,
                        pal.selection.1,
                        pal.selection.2,
                        200,
                    )
                } else {
                    Color32::from_rgba_unmultiplied(pal.hover.0, pal.hover.1, pal.hover.2, 160)
                };
                painter.rect_filled(rect, 4.0, fill);
            }

            // The real icon: `SHGetFileInfoW` resolves a `.lnk` to the picture of
            // the program it points at, so shortcuts look like shortcuts.
            let tex = self.icon_texture(ctx, icon);
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + 6.0 + icon_size / 2.0),
                egui::vec2(icon_size, icon_size),
            );
            match tex {
                Some(id) => {
                    painter.image(
                        id,
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    painter.text(
                        icon_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if icon.is_dir { "📁" } else { "📄" },
                        egui::FontId::proportional(icon_size * 0.75),
                        Color32::from_rgb(240, 220, 150),
                    );
                }
            }
            // Two lines, never more, with a real ellipsis on the second: a
            // caption longer than the icon's cell used to be wrapped without a
            // limit and spilled out of the selection square, so the desktop
            // showed a ragged paragraph under one icon ("текст ярлыка не
            // скрывается").  The caption is also clipped to the cell, which
            // catches the case where even two wrapped lines are too wide.
            let caption = winrust_core::i18n::wrap_capped(&icon.name, 13, 2);
            let clip = painter.with_clip_rect(egui::Rect::from_min_max(
                egui::pos2(rect.left() - 2.0, rect.bottom() - 26.0),
                egui::pos2(rect.right() + 2.0, rect.bottom() + 2.0),
            ));
            clip.text(
                egui::pos2(rect.center().x, rect.bottom() - 24.0),
                egui::Align2::CENTER_TOP,
                caption,
                egui::FontId::proportional(11.5),
                Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
            );

            if resp.clicked() {
                let mods = ui.ctx().input(|i| i.modifiers);
                act.clicked = Some((idx, mods.ctrl || mods.command, mods.shift));
            }
            if resp.double_clicked() {
                act.open = Some(icon.clone());
            }
            if resp.drag_started() {
                self.mode = crate::ShellMode::DraggingIcon(idx);
                self.selected = Some(idx);
            }
            if let crate::ShellMode::DraggingIcon(drag_idx) = self.mode {
                if drag_idx == idx {
                    // Snap preview: outline the cell the icon would land in.
                    if let Some(p) = pointer {
                        let c = (
                            ((p.x - area.left()) / cell.x).max(0.0) as usize,
                            ((p.y - area.top()) / cell.y).max(0.0) as usize,
                        );
                        drop_off_cell = Some(c);
                        let target = egui::Rect::from_min_size(
                            area.left_top() + egui::vec2(c.0 as f32 * cell.x, c.1 as f32 * cell.y),
                            egui::vec2(cell.x - 6.0, cell.y - 8.0),
                        );
                        painter.rect_stroke(
                            target,
                            4.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(96, 176, 255)),
                        );
                        // The dragged picture itself, translucent under the
                        // cursor: Explorer's drag image.
                        let ghost = egui::Rect::from_center_size(
                            p - egui::vec2(0.0, 18.0),
                            egui::vec2(icon_size * 0.8, icon_size * 0.8),
                        );
                        if let Some(id) = tex {
                            painter.image(
                                id,
                                ghost,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::from_white_alpha(140),
                            );
                        } else {
                            painter.text(
                                ghost.center(),
                                egui::Align2::CENTER_CENTER,
                                if icon.is_dir { "📁" } else { "📄" },
                                egui::FontId::proportional(icon_size * 0.6),
                                Color32::from_white_alpha(140),
                            );
                        }
                        // A short label follows the icon, like Explorer's.
                        let label_pos = ghost.center_bottom() + egui::vec2(0.0, 2.0);
                        let label_rect = egui::Rect::from_center_size(
                            label_pos + egui::vec2(0.0, 7.0),
                            egui::vec2(cell.x - 10.0, 16.0),
                        );
                        painter.rect_filled(
                            label_rect,
                            3.0,
                            Color32::from_rgba_unmultiplied(
                                pal.selection.0,
                                pal.selection.1,
                                pal.selection.2,
                                190,
                            ),
                        );
                        painter.text(
                            label_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            winrust_core::i18n::end(&icon.name, 16),
                            egui::FontId::proportional(11.0),
                            Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
                        );
                    }
                    if drag_stopped {
                        if let (Some(p), Some(c)) = (pointer, drop_off_cell) {
                            if self.settings.snap_to_grid {
                                // A cell can only hold one icon: say so instead
                                // of dropping the icon on top of its neighbour.
                                let taken = icons
                                    .iter()
                                    .any(|other| other.cell == c && other.name != icon.name);
                                if taken {
                                    self.status = occupied_note(&icon.name);
                                } else {
                                    act.moved = Some((icon.name.clone(), c, None));
                                }
                            } else {
                                // Free placement: the icon stays exactly where
                                // it was let go.  `cell` is still updated, so
                                // turning the grid back on puts it in a sane
                                // place rather than wherever it started.
                                let free = (
                                    ((p.x - area.left()) / cell.x).max(0.0),
                                    ((p.y - area.top()) / cell.y).max(0.0),
                                );
                                act.moved = Some((icon.name.clone(), c, Some(free)));
                            }
                        }
                        self.mode = crate::ShellMode::Idle;
                    }
                }
            }
            resp.context_menu(|ui| {
                if ui.button(t("Open")).clicked() {
                    act.open = Some(icon.clone());
                    ui.close_menu();
                }
                if ui.button(t("Rename")).clicked() {
                    // Explorer opens an inline editor; here the name is edited in
                    // the taskbar status line's dialog-free flow: a rename via the
                    // shell properties dialog.
                    let _ = winrust_core::shell::show_properties(&icon.path);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Cut")).clicked() {
                    self.clipboard = Some(icon.path.clone());
                    self.clipboard_cut = true;
                    ui.close_menu();
                }
                if ui.button(t("Copy")).clicked() {
                    self.clipboard = Some(icon.path.clone());
                    self.clipboard_cut = false;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Delete")).clicked() {
                    let _ = winrust_core::shell::recycle(std::slice::from_ref(&icon.path));
                    act.refresh = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Properties")).clicked() {
                    let _ = winrust_core::shell::show_properties(&icon.path);
                    ui.close_menu();
                }
            });
            resp.on_hover_text(format!(
                "{}\n{}\n{}",
                icon.name,
                type_text(icon.is_dir, &icon.path),
                icon.path.display()
            ));
        }

        // Rubber-band selection: press on the wallpaper starts a box, release
        // selects every icon it touched.  A plain click clears the selection.
        let (up, drag_started) =
            ctx.input(|i| (i.pointer.primary_released(), i.pointer.primary_pressed()));
        // Start a band only when the press landed on bare wallpaper.  `bg` spans
        // the whole desktop, so `hovered()` alone would also start a box when the
        // press was on an icon (egui only gives the *top* widget the interaction,
        // but `hovered()` is still true for the one underneath).
        if bg.hovered()
            && bg.is_pointer_button_down_on()
            && drag_started
            && self.band_start.is_none()
        {
            self.band_start = pointer;
        }
        if let Some(start) = self.band_start {
            if let Some(p) = pointer {
                let band = egui::Rect::from_two_pos(start, p);
                if band.width() > 4.0 || band.height() > 4.0 {
                    let painter = ctx.layer_painter(layer);
                    painter.rect_filled(
                        band,
                        0.0,
                        Color32::from_rgba_unmultiplied(
                            pal.selection.0,
                            pal.selection.1,
                            pal.selection.2,
                            60,
                        ),
                    );
                    painter.rect_stroke(
                        band,
                        0.0,
                        egui::Stroke::new(
                            1.0_f32,
                            Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2),
                        ),
                    );
                }
            }
        }
        let mods = ctx.input(|i| i.modifiers);
        if up {
            if let Some(start) = self.band_start.take() {
                if let Some(p) = pointer {
                    let band = egui::Rect::from_two_pos(start, p);
                    if band.width() > 4.0 || band.height() > 4.0 {
                        // Everything the box touches becomes selected.
                        if !mods.ctrl && !mods.command {
                            self.selection.clear();
                        }
                        for (idx, icon) in icons.iter().enumerate() {
                            let pos = area.left_top()
                                + egui::vec2(
                                    icon.cell.0 as f32 * cell.x,
                                    icon.cell.1 as f32 * cell.y,
                                );
                            let r = egui::Rect::from_min_size(
                                pos,
                                egui::vec2(cell.x - 6.0, cell.y - 8.0),
                            );
                            if band.intersects(r) {
                                self.selection.insert(idx);
                            }
                        }
                        // Keep the legacy single-selection field in step so
                        // rename/properties act on the band's first icon.
                        self.selected = self.selection.iter().next().copied();
                    } else if bg.clicked() && !mods.ctrl {
                        self.selection.clear();
                        self.selected = None;
                    }
                }
            }
        }
        if bg.clicked() && self.band_start.is_none() {
            if !mods.ctrl && !mods.command {
                self.selection.clear();
                self.selected = None;
            }
            self.close_start();
        }
        if let Some((idx, ctrl, _shift)) = act.clicked {
            if ctrl {
                if !self.selection.remove(&idx) {
                    self.selection.insert(idx);
                }
            } else {
                self.selection.clear();
                self.selection.insert(idx);
            }
            // Keep the old single-selection field in step for the code that
            // still reads it (drag, rename, properties).
            self.selected = self.selection.iter().next().copied();
        }
        // Dropping a file from Explorer onto the wallpaper: paste if we have
        // something on the clipboard, which is how Explorer behaves.
        if bg.hovered() && drag_stopped && self.mode == crate::ShellMode::Idle {
            if let Some(src) = self.clipboard.clone() {
                self.paste_into_desktop(&src);
                act.refresh = true;
            }
        }
        let _ = drag_delta;

        if let Some(icon) = act.open {
            // Virtual entries open through their namespace path; everything else
            // is a normal ShellExecute on the file.
            match icon.shell_path {
                Some(shell) => self.open_shell(shell),
                None => self.open_path(&icon.path),
            }
        }
        if act.snap_now {
            // Switching the grid back on tidies the desktop: every icon whose
            // free position was recorded moves to the nearest cell.  Doing it
            // here, once, is better than snapping every frame, because a
            // user-placed icon must stay put while the grid is off.
            let mut used: std::collections::HashSet<(usize, usize)> =
                std::collections::HashSet::new();
            for icon in self.icons.iter_mut() {
                let want = match icon.free {
                    Some((fx, fy)) => (fx.round() as usize, fy.round() as usize),
                    None => icon.cell,
                };
                let mut slot = want;
                // Find the nearest free cell in the same column, then walk on.
                let mut guard = 0;
                while used.contains(&slot) && guard < 4096 {
                    slot.1 += 1;
                    if slot.1 > 40 {
                        slot.1 = 0;
                        slot.0 += 1;
                    }
                    guard += 1;
                }
                used.insert(slot);
                icon.cell = slot;
                icon.free = None;
            }
            save_positions(&self.icons);
        }
        if let Some((name, c, free)) = act.moved {
            if let Some(icon) = self.icons.iter_mut().find(|i| i.name == name) {
                icon.cell = c;
                if free.is_some() {
                    icon.free = free;
                }
            }
            save_positions(&self.icons);
        }
        if let Some(size) = act.size {
            self.icon_size = size;
            self.settings_dirty = true;
        }
        if let Some(order) = act.sort {
            self.sort_order = order;
            self.reload_icons();
        }
        if act.order_toggled {
            self.reload_icons();
        }
        if act.new_folder {
            self.create_desktop_folder();
        }
        if let Some(ext) = act.new_file {
            self.create_desktop_file(&ext);
        }
        if act.refresh {
            self.reload_icons();
        }
        if self.mode == crate::ShellMode::DraggingIcon(usize::MAX) {
            self.mode = crate::ShellMode::Idle;
        }
    }
}

/// The type text shown in the icon's tooltip, using the shell's vocabulary.
pub fn type_text(is_dir: bool, path: &std::path::Path) -> String {
    if is_dir {
        return ts("File folder").to_string();
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    winrust_core::shell::type_name_for_extension(&ext, false)
}

/// `"{name} is already in this spot"`-style note for the status strip.
pub fn occupied_note(name: &str) -> String {
    tf!("{} is already in this spot", name)
}

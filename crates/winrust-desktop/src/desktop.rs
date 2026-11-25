//! Desktop icons: the user's Desktop folder, drawn as a grid.
//!
//! Windows' own desktop is a list view over `%USERPROFILE%\Desktop` plus the
//! two virtual entries (This PC and the Recycle Bin).  We do the same, but the
//! grid position is persisted in `%APPDATA%\WinRust\desktop.json`, so icons stay
//! where the user puts them across sign-ins — the shell must not shuffle the
//! desktop every time it starts.

use eframe::egui;
use egui::Color32;
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
}

pub const CELL: egui::Vec2 = egui::vec2(96.0, 92.0);

fn positions_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(base).join("WinRust");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("desktop.json"))
}

fn load_positions() -> std::collections::HashMap<String, (usize, usize)> {
    positions_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_positions(icons: &[DesktopIcon]) {
    let Some(path) = positions_path() else {
        return;
    };
    let map: std::collections::HashMap<&str, (usize, usize)> =
        icons.iter().map(|i| (i.name.as_str(), i.cell)).collect();
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, text);
    }
}

/// Enumerate the desktop: two virtual entries first, then the folder contents.
pub fn scan() -> Vec<DesktopIcon> {
    let mut out: Vec<DesktopIcon> = Vec::new();
    let positions = load_positions();

    out.push(DesktopIcon {
        name: ts("This PC").to_string(),
        path: KnownFolder::Computer
            .path()
            .unwrap_or_else(|| PathBuf::from("C:\\")),
        is_dir: true,
        cell: (0, 0),
    });
    out.push(DesktopIcon {
        name: ts("Recycle Bin").to_string(),
        path: KnownFolder::RecycleBin
            .path()
            .unwrap_or_else(|| PathBuf::from("C:\\")),
        is_dir: true,
        cell: (0, 1),
    });

    if let Some(desktop) = KnownFolder::Desktop.path() {
        if let Ok(read) = std::fs::read_dir(&desktop) {
            let mut entries: Vec<_> = read.flatten().collect();
            entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let is_dir = e.metadata().map(|m| m.is_dir()).unwrap_or(false);
                out.push(DesktopIcon {
                    path: e.path(),
                    name,
                    is_dir,
                    cell: (0, 0),
                });
            }
        }
    }

    // Restore saved positions, then fill the remaining cells column by column.
    let mut taken: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for (idx, icon) in out.iter_mut().enumerate() {
        if let Some(cell) = positions.get(&icon.name) {
            if taken.insert(*cell) {
                icon.cell = *cell;
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

impl DesktopApp {
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

        let mut open: Option<PathBuf> = None;
        let mut moved: Option<(String, (usize, usize))> = None;
        let mut clicked: Option<usize> = None;
        let mut drop_off_cell: Option<(usize, usize)> = None;

        // The wallpaper hit area is registered before the icons, because inside a
        // single layer the widget registered last wins the hit test — icons must
        // stay clickable.  Clicking empty wallpaper clears the selection and
        // closes Start.
        let bg = ui.interact(area, egui::Id::new("desktop_bg"), egui::Sense::click());

        let icons = self.icons.clone();
        let selected_idx = self.selected;
        let (pointer, drag_stopped) =
            ctx.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));

        for (idx, icon) in icons.iter().enumerate() {
            let pos = area.left_top()
                + egui::vec2(icon.cell.0 as f32 * CELL.x, icon.cell.1 as f32 * CELL.y);
            let rect = egui::Rect::from_min_size(pos, egui::vec2(CELL.x - 6.0, CELL.y - 8.0));
            let resp = ui.interact(
                rect,
                egui::Id::new(("desktop_icon", idx)),
                egui::Sense::click_and_drag(),
            );
            let selected = selected_idx == Some(idx);
            let painter = ctx.layer_painter(layer);
            if resp.hovered() || selected {
                painter.rect_filled(
                    rect,
                    4.0,
                    if selected {
                        Color32::from_rgba_unmultiplied(48, 96, 168, 160)
                    } else {
                        Color32::from_rgba_unmultiplied(40, 52, 72, 110)
                    },
                );
            }
            let glyph = if icon.is_dir { "📁" } else { "📄" };
            painter.text(
                rect.center_top() + egui::vec2(0.0, 22.0),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(34.0),
                Color32::from_rgb(240, 220, 150),
            );
            painter.text(
                egui::pos2(rect.center().x, rect.bottom() - 24.0),
                egui::Align2::CENTER_TOP,
                winrust_core::i18n::wrap(&icon.name, 13).join("\n"),
                egui::FontId::proportional(11.5),
                Color32::from_rgb(232, 238, 248),
            );

            if resp.clicked() {
                clicked = Some(idx);
            }
            if resp.double_clicked() {
                open = Some(icon.path.clone());
            }
            if resp.drag_started() {
                self.mode = crate::ShellMode::DraggingIcon(idx);
                self.selected = Some(idx);
            }
            if let crate::ShellMode::DraggingIcon(drag_idx) = self.mode {
                if drag_idx == idx {
                    // Snap preview: outline the cell the icon would land in.
                    if let Some(p) = pointer {
                        let cell = (
                            ((p.x - area.left()) / CELL.x).max(0.0) as usize,
                            ((p.y - area.top()) / CELL.y).max(0.0) as usize,
                        );
                        drop_off_cell = Some(cell);
                        let target = egui::Rect::from_min_size(
                            area.left_top()
                                + egui::vec2(cell.0 as f32 * CELL.x, cell.1 as f32 * CELL.y),
                            egui::vec2(CELL.x - 6.0, CELL.y - 8.0),
                        );
                        painter.rect_stroke(
                            target,
                            4.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(96, 176, 255)),
                        );
                    }
                    if drag_stopped {
                        if let (Some(_p), Some(cell)) = (pointer, drop_off_cell) {
                            // A cell can only hold one icon: say so instead of
                            // dropping the icon on top of its neighbour.
                            let taken = icons
                                .iter()
                                .any(|other| other.cell == cell && other.name != icon.name);
                            if taken {
                                self.status = occupied_note(&icon.name);
                            } else {
                                moved = Some((icon.name.clone(), cell));
                            }
                        }
                        self.mode = crate::ShellMode::Idle;
                    }
                }
            }
            resp.context_menu(|ui| {
                if ui.button(t("Open")).clicked() {
                    open = Some(icon.path.clone());
                    ui.close_menu();
                }
                if ui.button(t("Properties")).clicked() {
                    let _ = winrust_core::shell::show_properties(&icon.path);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Delete")).clicked() {
                    let _ = winrust_core::shell::recycle(std::slice::from_ref(&icon.path));
                    ui.close_menu();
                }
            });
            resp.on_hover_text(format!(
                "{}\n{}",
                icon.name,
                type_text(icon.is_dir, &icon.path)
            ));
        }

        if bg.clicked() {
            self.selected = None;
            self.close_start();
        } else if let Some(idx) = clicked {
            self.selected = Some(idx);
        }
        if let Some(path) = open {
            self.open_path(&path);
        }
        if let Some((name, cell)) = moved {
            if let Some(icon) = self.icons.iter_mut().find(|i| i.name == name) {
                icon.cell = cell;
            }
            save_positions(&self.icons);
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

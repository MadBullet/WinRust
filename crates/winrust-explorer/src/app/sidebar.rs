//! The navigation pane's two sections: This PC's drives, and the folders
//! the user pinned.

use super::state::ExplorerApp;
use eframe::egui;
use egui::Color32;
use egui::RichText;
use std::path::PathBuf;
use winrust_core::disk;
use winrust_core::i18n;
use winrust_core::shell::KnownFolder;
use winrust_core::t;
use winrust_core::tf;
use winrust_core::ts;

impl ExplorerApp {
    pub(super) fn drives_section(&mut self, ui: &mut egui::Ui) {
        if self.drives.is_empty() {
            self.drives = disk::volumes();
        }
        if self.drives.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let count = self.drives.len();
        ui.label(
            RichText::new(format!("{} ({count})", t("Devices and drives")))
                .size(14.0)
                .strong(),
        );
        ui.add_space(4.0);
        let mut nav: Option<PathBuf> = None;
        // Tiles wrap like Explorer's device view: 260 px each.
        let per_row = ((ui.available_width() + 12.0) / 272.0).floor().max(1.0) as usize;
        for chunk in self.drives.clone().chunks(per_row) {
            ui.horizontal(|ui| {
                for drive in chunk {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(260.0, 74.0), egui::Sense::click());
                    let hovered = resp.hovered();
                    let painter = ui.painter();
                    painter.rect_filled(
                        rect,
                        6.0,
                        if hovered {
                            Color32::from_rgb(40, 44, 56)
                        } else {
                            Color32::from_rgb(32, 34, 44)
                        },
                    );
                    // Drive glyph
                    painter.text(
                        rect.left_top() + egui::vec2(14.0, 18.0),
                        egui::Align2::LEFT_TOP,
                        "▤",
                        egui::FontId::proportional(28.0),
                        Color32::from_rgb(150, 190, 240),
                    );
                    let name = if drive.label.is_empty() {
                        format!(
                            "{} ({})",
                            drive.kind.map(|k| k.label()).unwrap_or(ts("Drive")),
                            drive.root.trim_end_matches('\\')
                        )
                    } else {
                        format!("{} ({})", drive.label, drive.root.trim_end_matches('\\'))
                    };
                    painter.text(
                        rect.left_top() + egui::vec2(54.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        i18n::end(&name, 30),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(226, 230, 240),
                    );
                    // Usage bar
                    let frac = if drive.total_bytes > 0 {
                        drive.used_percent() / 100.0
                    } else {
                        0.0
                    };
                    let bar = egui::Rect::from_min_size(
                        rect.left_top() + egui::vec2(54.0, 28.0),
                        egui::vec2(rect.width() - 70.0, 8.0),
                    );
                    painter.rect_filled(bar, 2.0, Color32::from_rgb(20, 22, 30));
                    let mut filled = bar;
                    filled.set_width(bar.width() * frac.clamp(0.0, 1.0));
                    let color = if frac > 0.9 {
                        Color32::from_rgb(226, 96, 96)
                    } else if frac > 0.75 {
                        Color32::from_rgb(226, 180, 96)
                    } else {
                        Color32::from_rgb(72, 140, 220)
                    };
                    painter.rect_filled(filled, 2.0, color);
                    // "117 ГБ свободно из 228 ГБ"
                    let text = if drive.total_bytes == 0 {
                        ts("Not ready").to_string()
                    } else {
                        tf!(
                            "{} free of {}",
                            disk::format_size(drive.free_bytes),
                            disk::format_size(drive.total_bytes)
                        )
                    };
                    painter.text(
                        rect.left_top() + egui::vec2(54.0, 42.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::proportional(11.5),
                        Color32::from_rgb(158, 168, 184),
                    );
                    // Open on double click, like every other tile; a single
                    // click only selects.  The old handler navigated on
                    // `clicked()`, so a double click opened the same folder twice
                    // and the view looked dead.
                    if resp.double_clicked() {
                        nav = Some(PathBuf::from(&drive.root));
                    }
                    if resp.clicked() {
                        self.status = tf!("{} selected", &name);
                    }
                    resp.context_menu(|ui| {
                        if ui.button(t("Open")).clicked() {
                            nav = Some(PathBuf::from(&drive.root));
                            ui.close_menu();
                        }
                        if ui.button(t("Properties")).clicked() {
                            let _ = winrust_core::shell::show_properties(std::path::Path::new(
                                &drive.root,
                            ));
                            ui.close_menu();
                        }
                    });
                    resp.on_hover_text(format!(
                        "{}\n{}\n{}",
                        name,
                        drive.fs_name,
                        if drive.total_bytes == 0 {
                            ts("Not ready").to_string()
                        } else {
                            tf!(
                                "{} free of {}",
                                disk::format_size(drive.free_bytes),
                                disk::format_size(drive.total_bytes)
                            )
                        }
                    ));
                }
            });
            ui.add_space(6.0);
        }
        if let Some(p) = nav {
            self.navigate(p);
        }
    }

    /// The folder tiles of "Папки (7)" — the section Explorer shows on This PC.
    pub(super) fn folders_section(&mut self, ui: &mut egui::Ui) {
        let known = [
            KnownFolder::Desktop,
            KnownFolder::Documents,
            KnownFolder::Downloads,
            KnownFolder::Pictures,
            KnownFolder::Music,
            KnownFolder::Videos,
        ];
        let mut nav: Option<PathBuf> = None;
        ui.add_space(6.0);
        let count = known.len();
        ui.label(
            RichText::new(format!("{} ({count})", t("Folders")))
                .size(14.0)
                .strong(),
        );
        ui.add_space(4.0);
        let per_row = ((ui.available_width() + 12.0) / 168.0).floor().max(1.0) as usize;
        for chunk in known.chunks(per_row) {
            ui.horizontal(|ui| {
                for folder in chunk {
                    let Some(path) = folder.path() else {
                        continue;
                    };
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(156.0, 34.0),
                        egui::Sense::click_and_drag(),
                    );
                    let painter = ui.painter();
                    if resp.hovered() {
                        painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 44, 56));
                    }
                    painter.text(
                        rect.left_center() + egui::vec2(4.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "📁",
                        egui::FontId::proportional(19.0),
                        Color32::from_rgb(240, 196, 92),
                    );
                    painter.text(
                        rect.left_center() + egui::vec2(30.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        i18n::end(folder.label(), 22),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(226, 230, 240),
                    );
                    if resp.clicked() {
                        nav = Some(path.clone());
                    }
                    // Drag a folder tile onto another folder (or onto the tree)
                    // to move it there.  `drag_started` seeds `drag_paths` with
                    // this single folder; the selection is deliberately not
                    // consulted, because a tile is its own drag source.
                    if resp.drag_started() {
                        self.drag_paths = vec![path.clone()];
                        self.drag_active = true;
                        self.drag_ghost = None;
                        ui.ctx().request_repaint();
                    }
                    // Folders are drop targets here too.  Highlight while the
                    // pointer is over them; commit the drop only on release, so a
                    // drag that ends anywhere else does not land in the last
                    // folder it passed over.
                    if self.drag_active && resp.hovered() {
                        painter.rect_stroke(
                            rect.expand(1.0),
                            4.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(96, 176, 255)),
                        );
                        if ui.ctx().input(|i| i.pointer.primary_released()) {
                            self.drop_target = Some(path.clone());
                        }
                    }
                    resp.on_hover_text(path.display().to_string());
                }
            });
        }
        if let Some(p) = nav {
            self.navigate(p);
        }
    }
}

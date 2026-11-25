//! The preview pane and the status bar along the bottom edge.

use super::state::{truncate_middle, ExplorerApp};
use super::tabs::file_name_of;
use super::viewmode::ViewMode;
use eframe::egui;
use egui::RichText;
use std::path::Path;
use winrust_core::disk::{self, DirEntry, SortKey};
use winrust_core::shell;
use winrust_core::t;

impl ExplorerApp {
    pub(super) fn preview_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(path) = self.current().selected_one().cloned() else {
            ui.add_space(10.0);
            ui.label(RichText::new(t("Select a file to preview")).weak());
            return;
        };
        ui.add_space(6.0);
        let name = file_name_of(&path);
        if let Some(id) = self.icon_for_path(ctx, &path, 48) {
            ui.add(egui::Image::new((id, egui::vec2(48.0, 48.0))));
        }
        ui.label(RichText::new(&name).strong());
        ui.separator();

        let meta = std::fs::metadata(&path).ok();
        if let Some(meta) = &meta {
            egui::Grid::new("preview_meta")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Type").weak());
                    let entry = self
                        .current()
                        .entries
                        .iter()
                        .find(|e| e.path == path)
                        .cloned();
                    let t = match entry {
                        Some(e) => self.type_text(&e),
                        None => "File".to_string(),
                    };
                    ui.label(t);
                    ui.end_row();
                    ui.label(RichText::new("Size").weak());
                    ui.label(disk::format_size(meta.len()));
                    ui.end_row();
                    if let Ok(modified) = meta.modified() {
                        if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                            ui.label(RichText::new("Modified").weak());
                            ui.label(disk::format_local(dur.as_secs()));
                            ui.end_row();
                        }
                    }
                    ui.label(RichText::new("Path").weak());
                    ui.label(
                        RichText::new(truncate_middle(&path.display().to_string(), 46))
                            .small()
                            .weak(),
                    );
                    ui.end_row();
                });
            ui.separator();
            // Text preview for small text files, like Explorer's preview pane.
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let is_text = matches!(
                ext.as_str(),
                "txt"
                    | "md"
                    | "log"
                    | "json"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "csv"
                    | "rs"
                    | "c"
                    | "h"
                    | "cpp"
                    | "hpp"
                    | "py"
                    | "js"
                    | "ts"
                    | "html"
                    | "css"
                    | "xml"
                    | "ini"
                    | "cfg"
                    | "conf"
                    | "sh"
                    | "bat"
                    | "ps1"
            );
            if is_text && meta.len() <= 64 * 1024 {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let head: String = text.lines().take(60).collect::<Vec<_>>().join("\n");
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(head).monospace().size(11.0))
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                        });
                }
            }
        }
    }

    /// Icon for an arbitrary path (used by the preview pane).
    pub(super) fn icon_for_path(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        size: u32,
    ) -> Option<egui::TextureId> {
        let key = format!("path:{size}:{}", path.display());
        if let Some(slot) = self.icons.get(&key) {
            return slot.as_ref().map(|t| t.id());
        }
        let info = shell::file_info_sized(path, size);
        let texture = info.icon_rgba.and_then(|rgba| {
            let expected = (size * size * 4) as usize;
            if rgba.len() < expected {
                return None;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [size as usize, size as usize],
                &rgba[..expected],
            );
            Some(ctx.load_texture(key.clone(), image, egui::TextureOptions::LINEAR))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.icons.insert(key, texture);
        id
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let tab = self.current();
            let files = tab.entries.iter().filter(|e| !e.is_dir).count();
            let dirs = tab.entries.iter().filter(|e| e.is_dir).count();
            let selected = tab.selection.len();
            let text = if selected > 0 {
                format!("{selected} item(s) selected")
            } else {
                let mut s = format!("{dirs} folder(s), {files} file(s)");
                if let Some((_, bytes, count)) = &self.folder_size {
                    if *count > 0 {
                        s.push_str(&format!(
                            "  ·  {} total in {}",
                            disk::format_size(*bytes),
                            plural(*count)
                        ));
                    }
                }
                s
            };
            ui.label(RichText::new(text).small());

            if !self.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.status).small().weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("WinRust {}", env!("CARGO_PKG_VERSION")))
                        .small()
                        .weak(),
                );
                ui.separator();
                // View-mode shortcuts, like Explorer's status-bar buttons.
                for v in ViewMode::ALL {
                    let active = self.current().view == v;
                    if ui
                        .add(
                            egui::Button::new(RichText::new(view_glyph(v)).size(13.0))
                                .frame(active),
                        )
                        .on_hover_text(v.label())
                        .clicked()
                    {
                        self.current_mut().view = v;
                        self.remember_prefs();
                    }
                }
            });
        });
    }
}

pub(super) fn plural(n: u64) -> String {
    format!("{n} item(s)")
}

pub(super) fn view_glyph(v: ViewMode) -> &'static str {
    match v {
        ViewMode::Details => "☰",
        ViewMode::List => "⋯",
        ViewMode::Tiles => "▦",
        ViewMode::Content => "▤",
    }
}

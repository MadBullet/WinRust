//! The chrome around the file area: the tab strip, the command bar and the
//! navigation pane.

use super::commands::Command;
use super::state::{truncate_middle, ExplorerApp, FileClipboard};
use super::tabs::{file_name_of, Special, Tab};
use super::viewmode::{SortKeySerde, ViewMode};
use eframe::egui;
use egui::Color32;
use egui::RichText;
use std::path::PathBuf;
use winrust_core::disk;
use winrust_core::shell;
use winrust_core::shell::KnownFolder;
use winrust_core::t;
use winrust_core::tf;
use winrust_core::ts;

impl ExplorerApp {
    /// Explorer's tab strip (Win11 style) with a "+" button.
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut activate: Option<usize> = None;
            let mut close: Option<usize> = None;
            for (idx, tab) in self.tabs.iter().enumerate() {
                let selected = idx == self.active;
                let title = truncate_middle(&tab.title(), 26);
                let resp = ui.selectable_label(selected, format!("  {title}  "));
                if resp.clicked() {
                    activate = Some(idx);
                }
                if resp.middle_clicked() {
                    close = Some(idx);
                }
                resp.on_hover_text(tab.path.display().to_string());
                if tab.loading {
                    ui.spinner();
                }
                // Close button on the active tab.
                if selected && self.tabs.len() > 1 && ui.small_button("×").clicked() {
                    close = Some(idx);
                }
                ui.separator();
            }
            if ui
                .button("+")
                .on_hover_text(t("New tab (Ctrl+T)"))
                .clicked()
            {
                let p = self.current().path.clone();
                self.tabs.push(Tab::new(p));
                activate = Some(self.tabs.len() - 1);
            }
            if let Some(idx) = activate {
                self.active = idx;
                self.remember_prefs();
                self.start_load(self.active);
            }
            if let Some(idx) = close {
                if self.tabs.len() > 1 {
                    self.tabs.remove(idx);
                    self.active = self.active.min(self.tabs.len() - 1);
                    self.start_load(self.active);
                }
            }
        });
    }

    /// Command bar: back/forward/up/refresh, breadcrumb address bar, search,
    /// then the file commands Explorer puts in its ribbon.
    pub(super) fn command_bar(&mut self, ui: &mut egui::Ui) {
        // Row 1: navigation + address + search
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.current().can_back(),
                    egui::Button::new(RichText::new("◀").size(14.0)),
                )
                .on_hover_text(t("Back (Alt+←)"))
                .clicked()
            {
                self.go_back();
            }
            if ui
                .add_enabled(
                    self.current().can_forward(),
                    egui::Button::new(RichText::new("▶").size(14.0)),
                )
                .on_hover_text(t("Forward (Alt+→)"))
                .clicked()
            {
                self.go_forward();
            }
            if ui.button("▲").on_hover_text(t("Up (Alt+↑)")).clicked() {
                self.go_up();
            }
            if ui.button("⟳").on_hover_text(t("Refresh (F5)")).clicked() {
                self.refresh();
            }

            let editing = self.current().address_edit.is_some();
            let search_w = 190.0;
            let addr_w = (ui.available_width() - search_w - 12.0).max(200.0);
            if editing {
                let mut buf = self.current().address_edit.clone().unwrap_or_default();
                let response = ui.add_sized(
                    [addr_w, 24.0],
                    egui::TextEdit::singleline(&mut buf)
                        .hint_text(t("Type a path or URL and press Enter")),
                );
                self.current_mut().address_edit = Some(buf.clone());
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.current_mut().address_edit = None;
                    let p = PathBuf::from(buf.trim());
                    if p.exists() {
                        self.navigate(p);
                    } else {
                        self.status = tf!("Path not found: {}", p.display());
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.current_mut().address_edit = None;
                }
            } else {
                let mut target: Option<PathBuf> = None;
                let mut start_edit = false;
                egui::ScrollArea::horizontal()
                    .id_salt("breadcrumb")
                    .max_width(addr_w)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // A virtual folder has no components worth
                            // showing: its `shell:` string would appear as
                            // one opaque crumb, so it names itself.
                            if let Some(special) = self.current().special {
                                ui.label(RichText::new(ts(special.title())).size(13.0));
                                return;
                            }
                            let path = self.current().path.clone();
                            let mut acc = PathBuf::new();
                            for (idx, comp) in path.components().enumerate() {
                                acc.push(comp);
                                let label = comp.as_os_str().to_string_lossy().to_string();
                                let text = if label.is_empty() {
                                    path.display().to_string()
                                } else if idx == 0 {
                                    label.trim_end_matches('\\').to_string()
                                } else {
                                    label
                                };
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(text).size(13.0))
                                            .frame(false),
                                    )
                                    .clicked()
                                {
                                    target = Some(acc.clone());
                                }
                                if acc != path {
                                    ui.label(RichText::new("›").weak());
                                }
                            }
                            if ui
                                .small_button("✎")
                                .on_hover_text(t("Edit address (Ctrl+L)"))
                                .clicked()
                            {
                                start_edit = true;
                            }
                        });
                    });
                if start_edit {
                    let p = self.current().path.display().to_string();
                    self.current_mut().address_edit = Some(p);
                }
                if let Some(p) = target {
                    self.navigate(p);
                }
            }

            // Search box
            let mut needle = self.current().filter.clone();
            let resp = ui.add_sized(
                [search_w, 24.0],
                egui::TextEdit::singleline(&mut needle)
                    .hint_text(format!("🔍 {}", t("Search")))
                    .margin(egui::vec2(6.0, 3.0)),
            );
            if resp.changed() {
                self.start_search(needle.clone());
            }
            if needle.is_empty() && self.search_results.is_some() {
                self.search_results = None;
            }
        });

        ui.add_space(2.0);

        // Row 2: the file commands
        ui.horizontal(|ui| {
            let entries = self.sorted_filtered();
            let multiselection = self.current().selection.clone();
            ui.menu_button(RichText::new("+ New").strong(), |ui| {
                if ui.button(format!("📁 {}", t("Folder"))).clicked() {
                    self.creating_folder = true;
                    self.new_folder_buffer = t("New folder");
                    ui.close_menu();
                }
                if ui.button(format!("📄 {}", t("Text document"))).clicked() {
                    let target = disk::unique_destination(
                        self.current().path.join(t("New Text Document.txt")),
                    );
                    match std::fs::write(&target, b"") {
                        Ok(()) => {
                            self.status = tf!("Created {}", file_name_of(&target));
                            self.refresh();
                        }
                        Err(e) => self.status = tf!("Create failed: {}", e),
                    }
                    ui.close_menu();
                }
                if ui
                    .button(format!("🗜 {}", t("Compressed (zipped) folder")))
                    .clicked()
                {
                    self.status = t("Use right-click → Send to → Compressed folder");
                    ui.close_menu();
                }
            });

            ui.separator();

            let has_sel = !multiselection.is_empty();
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("✂ {}", t("Cut"))))
                .on_hover_text("Ctrl+X")
                .clicked()
            {
                self.clipboard = Some(FileClipboard {
                    paths: multiselection.clone(),
                    cut: true,
                });
                self.status = tf!("Cut {} item(s)", multiselection.len());
            }
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("⧉ {}", t("Copy"))))
                .on_hover_text("Ctrl+C")
                .clicked()
            {
                self.clipboard = Some(FileClipboard {
                    paths: multiselection.clone(),
                    cut: false,
                });
                self.status = tf!("Copied {} item(s)", multiselection.len());
            }
            let has_clip = self.clipboard.is_some();
            if ui
                .add_enabled(has_clip, egui::Button::new(format!("📋 {}", t("Paste"))))
                .on_hover_text("Ctrl+V")
                .clicked()
            {
                self.paste_clipboard();
            }
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("✎ {}", t("Rename"))))
                .on_hover_text("F2")
                .clicked()
            {
                if let Some(one) = multiselection.first() {
                    self.rename_buffer = file_name_of(one);
                    self.rename_target = Some(one.clone());
                }
            }
            if ui
                .add_enabled(has_sel, egui::Button::new(format!("🗑 {}", t("Delete"))))
                .on_hover_text("Delete / Shift+Delete")
                .clicked()
            {
                self.delete_confirm = multiselection.clone();
                self.delete_permanent = false;
            }
            ui.separator();
            if ui
                .button(format!("⤴ {}", t("Share")))
                .on_hover_text("Open sharing options")
                .clicked()
            {
                if let Some(one) = multiselection.first() {
                    let _ = shell::open_verb(one, "share");
                } else {
                    self.status = t("Sharing: select an item first");
                }
            }
            ui.separator();

            // View / sort / options
            let mut view = self.current().view;
            egui::ComboBox::from_id_salt("viewmode")
                .selected_text(view.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for v in ViewMode::ALL {
                        if ui.selectable_value(&mut view, v, v.label()).clicked() {
                            self.current_mut().view = v;
                            self.remember_prefs();
                        }
                    }
                });

            let mut sort_key = self.current().sort_key;
            let sort_asc = self.current().sort_asc;
            egui::ComboBox::from_id_salt("sortby")
                .selected_text(format!("{}: {}", t("Sort"), sort_key.label()))
                .width(170.0)
                .show_ui(ui, |ui| {
                    for k in SortKeySerde::ALL {
                        if ui.selectable_value(&mut sort_key, k, k.label()).clicked() {
                            self.current_mut().sort_key = k;
                            self.current_mut().sort_asc = true;
                            self.remember_prefs();
                        }
                    }
                    ui.separator();
                    if ui.selectable_label(sort_asc, t("Ascending")).clicked() {
                        self.current_mut().sort_asc = true;
                    }
                    if ui.selectable_label(!sort_asc, t("Descending")).clicked() {
                        self.current_mut().sort_asc = false;
                    }
                });

            ui.menu_button(format!("⋯ {}", t("View")), |ui| {
                let mut hidden = self.current().show_hidden;
                if ui.checkbox(&mut hidden, t("Hidden items")).changed() {
                    self.current_mut().show_hidden = hidden;
                    self.remember_prefs();
                    self.refresh();
                }
                let mut prev = self.preview_open;
                if ui.checkbox(&mut prev, "Preview pane").changed() {
                    self.preview_open = prev;
                }
                ui.separator();
                if ui.button(t("Refresh (F5)")).clicked() {
                    self.refresh();
                    ui.close_menu();
                }
                if ui.button(t("Select all (Ctrl+A)")).clicked() {
                    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                    self.current_mut().selection = paths;
                    ui.close_menu();
                }
            });
        });
    }

    /// Navigation pane: Quick access, This PC (drives) with usage bars, and the
    /// sub-folders of the current directory (Explorer's tree, one level deep).
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut nav: Option<PathBuf> = None;
        let cur_path = self.current().path.clone();
        // Snapshot what the pane needs so `self` is not borrowed inside the
        // closure while it is also mutated by click handling.
        let mut nav_selected = self.nav_selected;
        let mut nav_special: Option<Special> = None;
        let mut background_cmd: Option<Command> = None;
        let drives = self.drives.clone();
        let pinned = self.pinned.clone();
        let mut unpin_now: Option<PathBuf> = None;
        let mut pin_now: Vec<PathBuf> = Vec::new();
        // Reordering the favourites list by drag: which row is being dragged
        // and which row it was dropped on.
        let mut fav_drag: Option<(usize, PathBuf)> = None;
        let mut fav_drop: Option<usize> = None;
        // Dropping a folder on the favourites area pins it.  `drag_paths` is the
        // snapshot the right pane filled in when the drag started.
        let dragged: Vec<PathBuf> = self.drag_paths.clone();
        let dragging = self.drag_active;
        // The pane's own background, so a right-click in the empty strip above
        // the folder list opens the same context menu the file area has - the
        // review's "there is nowhere to right-click in the left pane".  The
        // strip is allocated first so it exists even when the tree is short.
        let strip =
            ui.allocate_response(egui::vec2(ui.available_width(), 10.0), egui::Sense::click());
        // `refresh_now` is set instead of pushing a `Command`: the context menu
        // runs while `self` is borrowed for drawing, and `run(..)` needs
        // `&mut self`.
        let mut refresh_now = false;
        let mut new_folder = false;
        strip.context_menu(|ui| {
            ui.set_min_width(220.0);
            if ui.button(t("Refresh")).clicked() {
                refresh_now = true;
                ui.close_menu();
            }
            if ui.button(t("Paste")).clicked() {
                background_cmd = Some(Command::Paste);
                ui.close_menu();
            }
            if ui.button(t("New folder")).clicked() {
                // The same flag the "+ New > Folder" button sets; the file
                // area draws the inline name field for it.
                new_folder = true;
                ui.close_menu();
            }
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(t("QUICK ACCESS")).small().weak());
                for folder in KnownFolder::sidebar() {
                    let label = folder.label();
                    let path = folder.path();
                    let active = match &path {
                        Some(p) => cur_path == *p,
                        None => nav_selected == Some(*folder),
                    };
                    let text =
                        RichText::new(format!("{}  {}", sidebar_glyph(*folder), label)).size(13.0);
                    let text = if active { text.strong() } else { text };
                    if ui.add(egui::Button::new(text).frame(active)).clicked() {
                        nav_selected = Some(*folder);
                        match path {
                            Some(p) => nav = Some(p),
                            // This PC, Network and the Recycle Bin have no
                            // path; the tab shows them as a virtual folder
                            // instead of a status-bar complaint.
                            None => nav_special = Special::of(*folder),
                        }
                    }
                }

                ui.add_space(10.0);
                ui.label(RichText::new(t("THIS PC")).small().weak());
                for drive in &drives {
                    let p = PathBuf::from(&drive.root);
                    let name = if drive.label.is_empty() {
                        format!(
                            "{} ({})",
                            drive.kind.map(|k| k.label()).unwrap_or(ts("Drive")),
                            drive.root.trim_end_matches('\\')
                        )
                    } else {
                        format!("{} ({})", drive.label, drive.root.trim_end_matches('\\'))
                    };
                    let frac = if drive.total_bytes > 0 {
                        drive.free_bytes as f32 / drive.total_bytes as f32
                    } else {
                        0.0
                    };
                    let active = cur_path.starts_with(&p);
                    let text = RichText::new(format!("💽  {name}")).size(13.0);
                    let text = if active { text.strong() } else { text };
                    let resp = ui.add(egui::Button::new(text).frame(active));
                    if resp.clicked() {
                        nav = Some(p);
                    }
                    resp.on_hover_text(format!(
                        "{} · free {} of {}",
                        drive.fs_name,
                        disk::format_size(drive.free_bytes),
                        disk::format_size(drive.total_bytes)
                    ));
                    if drive.total_bytes > 0 {
                        ui.add(
                            egui::ProgressBar::new(1.0 - frac)
                                .desired_width((ui.available_width() - 8.0).max(40.0))
                                .desired_height(3.0)
                                .fill(Color32::from_rgb(0, 120, 215)),
                        );
                    }
                }

                ui.add_space(10.0);
                // Favourites, not "folders in this view".  The old list just
                // echoed the right pane, which is what the report asked to
                // remove; this one is the user's own and it is filled by
                // dragging a folder from the right pane onto it.
                ui.label(RichText::new(t("FAVOURITES")).small().weak());
                if pinned.is_empty() {
                    ui.label(
                        RichText::new(t("   drag a folder here to pin it"))
                            .size(11.0)
                            .weak(),
                    );
                }
                let mut unpin: Option<PathBuf> = None;
                for (idx, path) in pinned.iter().enumerate() {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    let here = *path == cur_path;
                    // A favourites row is a drag source *within its own list*:
                    // dragging it onto another row reorders the list, which is
                    // what "перетаскивать закреплённое" asks for.  Dragging a
                    // folder in from the right pane is a different gesture and
                    // still pins, see the drop zone below.
                    let resp = ui.add(
                        egui::Button::new(RichText::new(format!("📌  {name}")).size(12.0))
                            .frame(here)
                            .sense(egui::Sense::click_and_drag()),
                    );
                    if resp.clicked() {
                        nav = Some(path.clone());
                    }
                    if resp.drag_started() {
                        // A favourites row is not a file drag: `drag_active`
                        // stays false so the ghost and the drop zones in the
                        // right pane do not engage.
                        fav_drag = Some((idx, path.clone()));
                        ui.ctx().request_repaint();
                    }
                    if resp.hovered() && fav_drag.is_some() {
                        // Paint the insertion marker: the row the pointer is
                        // over receives the dragged favourite in its place.
                        let line_y = if fav_drag.map(|(f, _)| f) < Some(idx) {
                            resp.rect.bottom()
                        } else {
                            resp.rect.top()
                        };
                        ui.painter().hline(
                            resp.rect.x_range(),
                            line_y,
                            egui::Stroke::new(2.0_f32, Color32::from_rgb(96, 176, 255)),
                        );
                        if ui.ctx().input(|i| i.pointer.primary_released()) {
                            fav_drop = Some(idx);
                        }
                    }
                    if resp.secondary_clicked() {
                        unpin = Some(path.clone());
                    }
                    resp.on_hover_text(format!(
                        "{}\n{}",
                        path.display(),
                        t("Right-click to unpin")
                    ));
                }
                if let Some(path) = unpin {
                    unpin_now = Some(path);
                }
                // The drop target for "pin this folder".  Allocated after the
                // entries so it can own the leftover height, which makes the
                // whole bottom of the pane a target rather than one thin row.
                // Expanding it by a few pixels horizontally keeps the drop
                // alive when the pointer is right on the pane's edge.
                let drop_rect = egui::Rect::from_min_max(
                    ui.cursor().min,
                    egui::pos2(
                        ui.max_rect().right(),
                        (ui.cursor().top() + 60.0).max(ui.max_rect().bottom()),
                    ),
                );
                if dragging {
                    let hovering = ui.rect_contains_pointer(drop_rect);
                    ui.painter().rect_filled(
                        drop_rect,
                        3.0,
                        Color32::from_rgba_unmultiplied(
                            0,
                            120,
                            215,
                            if hovering { 56 } else { 20 },
                        ),
                    );
                    ui.painter().rect_stroke(
                        drop_rect,
                        3.0,
                        egui::Stroke::new(1.0_f32, Color32::from_rgb(0, 150, 255)),
                    );
                    if hovering {
                        ui.painter().text(
                            drop_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            t("Drop to pin"),
                            egui::FontId::proportional(11.0),
                            Color32::from_rgb(200, 230, 255),
                        );
                    }
                    // On release over the area, every dragged *folder* is
                    // pinned.  Files are skipped: a favourites list of files is
                    // a different feature and pinning one would be a surprise.
                    if hovering && ui.input(|i| i.pointer.primary_released()) {
                        for path in &dragged {
                            if path.is_dir() && !pinned.iter().any(|p| p == path) {
                                pin_now.push(path.clone());
                            }
                        }
                    }
                }
            });
        let _ = ctx;
        self.nav_selected = nav_selected;
        if let Some(to) = fav_drop {
            // Reorder in place; the saved order is the user's order.
            if let Some(from) = fav_drag.as_ref().map(|(i, _)| *i) {
                if from < self.pinned.len() && to < self.pinned.len() && from != to {
                    let moved = self.pinned.remove(from);
                    self.pinned.insert(to, moved);
                }
            }
        }
        if let Some(p) = unpin_now {
            self.pinned.retain(|x| x != &p);
            self.status = format!("{}: {}", t("Unpinned"), p.display());
        }
        if !pin_now.is_empty() {
            let count = pin_now.len();
            for p in pin_now {
                if !self.pinned.contains(&p) {
                    self.pinned.push(p);
                }
            }
            self.status = format!("{} {}", count, t("folder(s) pinned"));
        }
        if let Some(p) = nav {
            self.navigate(p);
        }
        if let Some(special) = nav_special {
            self.navigate_special(special);
        }
        // The context menu only records intent: `self` is borrowed by the
        // tree drawing above, and these three actions all need `&mut self`.
        if new_folder {
            self.creating_folder = true;
            self.new_folder_buffer = t("New folder");
        }
        if refresh_now {
            self.refresh();
        }
        if let Some(cmd) = background_cmd {
            let here = self.current().path.clone();
            self.run(here, cmd);
        }
    }
}

pub(super) fn sidebar_glyph(folder: KnownFolder) -> &'static str {
    match folder {
        KnownFolder::Profile => "🏠",
        KnownFolder::Desktop => "🖥",
        KnownFolder::Downloads => "⬇",
        KnownFolder::Documents => "📄",
        KnownFolder::Pictures => "🖼",
        KnownFolder::Music => "🎵",
        KnownFolder::Videos => "🎬",
        KnownFolder::Computer => "💻",
        KnownFolder::Network => "🌐",
        KnownFolder::RecycleBin => "🗑",
        _ => "📁",
    }
}

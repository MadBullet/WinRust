//! The four file views - details, list, tiles, content - and the drop
//! handling they share.

use super::commands::Command;
use super::state::{truncate_middle, ExplorerApp};
use super::tabs::file_name_of;
use super::viewmode::{SortKeySerde, ViewMode};
use eframe::egui;
use egui::Color32;
use egui::RichText;
use egui_extras::Column;
use egui_extras::TableBuilder;
use std::path::Path;
use std::path::PathBuf;
use winrust_core::disk::{self, DirEntry};
use winrust_core::shell::KnownFolder;
use winrust_core::t;
use winrust_core::tf;

impl ExplorerApp {
    pub(super) fn file_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(err) = self.current().error.clone() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("⚠")
                        .size(28.0)
                        .color(Color32::from_rgb(240, 160, 90)),
                );
                ui.label(RichText::new(&err).color(Color32::from_rgb(240, 140, 120)));
                ui.add_space(6.0);
                if ui.button(t("Retry")).clicked() {
                    self.refresh();
                }
                if ui.button(t("Up one level")).clicked() {
                    self.go_up();
                }
            });
            return;
        }

        if self.creating_folder {
            ui.horizontal(|ui| {
                ui.label(t("New folder name:"));
                let resp = ui.text_edit_singleline(&mut self.new_folder_buffer);
                if !resp.has_focus() {
                    resp.request_focus();
                }
                let commit = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(t("Create")).clicked() || commit {
                    let target =
                        disk::unique_destination(self.current().path.join(&self.new_folder_buffer));
                    match disk::create_dir_all(&target) {
                        Ok(()) => {
                            self.status = tf!("Created {}", file_name_of(&target));
                            self.creating_folder = false;
                            self.refresh();
                        }
                        Err(e) => self.status = tf!("Create failed: {}", e),
                    }
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.creating_folder = false;
                }
            });
            ui.separator();
        }

        if let Some(target) = self.rename_target.clone() {
            // Reborrow: the closure below is `FnOnce`, so capturing `self`
            // directly would move the `&mut` and lock the rest of the function.
            let this = &mut *self;
            ui.horizontal(|ui| {
                ui.label(tf!("Rename {}:", file_name_of(&target)));
                let resp = ui.text_edit_singleline(&mut this.rename_buffer);
                if !resp.has_focus() {
                    resp.request_focus();
                }
                let commit = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(t("Rename")).clicked() || commit {
                    let new_name = this.rename_buffer.trim().to_string();
                    let dest = target.with_file_name(&new_name);
                    match disk::move_path(&target, &dest, false) {
                        Ok(()) => {
                            this.status = tf!("Renamed to {}", new_name);
                            this.rename_target = None;
                            this.refresh();
                        }
                        Err(e) => this.status = tf!("Rename failed: {}", e),
                    }
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    this.rename_target = None;
                }
            });
            ui.separator();
        }

        // "This PC" gets the Explorer home layout: folder tiles on top, then
        // Devices and drives with usage bars and the free-of-total text.
        if self.is_computer_view() && self.current().filter.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("computer_home")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.folders_section(ui);
                    self.drives_section(ui);
                });
            return;
        }

        let entries = self.sorted_filtered();
        if entries.is_empty() && !self.current().loading {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                if self.current().filter.is_empty() {
                    ui.label(RichText::new(t("This folder is empty")).size(15.0).weak());
                } else {
                    ui.label(
                        RichText::new(t("No items match your search"))
                            .size(15.0)
                            .weak(),
                    );
                }
            });
            return;
        }

        match self.current().view {
            ViewMode::Details => self.details_view(ui, ctx, &entries),
            ViewMode::List => self.list_view(ui, ctx, &entries),
            ViewMode::Tiles => {
                let px = self.current().icon_px;
                self.tiles_view(ui, ctx, &entries, px)
            }
            ViewMode::Content => self.content_view(ui, ctx, &entries),
        }
    }

    /// Is the active tab showing "This PC" - the Computer known folder?
    ///
    /// This used to also answer "yes" for a bare drive root (`C:\`), which is
    /// the 0.9.0 bug the review found: opening a drive navigated the tab to
    /// `C:\`, the view recognised that as "This PC" again, and the same grid
    /// of drive tiles came back - so a double-click on a drive looked like it
    /// did nothing.  A drive root is a folder to list, not the Computer view.
    pub(super) fn is_computer_view(&self) -> bool {
        let p = &self.current().path;
        match KnownFolder::Computer.path() {
            Some(computer) => p == &computer,
            None => false,
        }
    }

    /// Shared selection logic: plain click replaces, Ctrl toggles, Shift extends.
    pub(super) fn apply_click(
        &mut self,
        index: usize,
        path: &Path,
        modifiers: egui::Modifiers,
        entries: &[DirEntry],
    ) {
        let ctrl = modifiers.ctrl || modifiers.command;
        let shift = modifiers.shift;
        let tab = self.current_mut();
        if shift {
            if let Some(anchor) = tab.anchor {
                let (lo, hi) = if anchor <= index {
                    (anchor, index)
                } else {
                    (index, anchor)
                };
                tab.selection = entries[lo..=hi].iter().map(|e| e.path.clone()).collect();
                return;
            }
        }
        if ctrl {
            if let Some(pos) = tab.selection.iter().position(|p| p == path) {
                tab.selection.remove(pos);
            } else {
                tab.selection.push(path.to_path_buf());
            }
            tab.anchor = Some(index);
        } else {
            tab.selection = vec![path.to_path_buf()];
            tab.anchor = Some(index);
        }
        // The "Open with" list belongs to the file, so it is rebuilt when the
        // selection changes, not every frame.
        self.open_with = self
            .current()
            .selection
            .first()
            .filter(|p| p.extension().is_some())
            .map(|p| winrust_core::fileassoc::open_with(p))
            .unwrap_or_default();
    }

    pub(super) fn details_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entries: &[DirEntry],
    ) {
        let app_list = self.open_with.clone();
        let sort_key = self.current().sort_key;
        let sort_asc = self.current().sort_asc;
        let clicked_cell = std::cell::Cell::new(None::<SortKeySerde>);

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(26.0).at_least(24.0).clip(true))
            .column(
                Column::initial(320.0)
                    .at_least(140.0)
                    .clip(true)
                    .resizable(true)
                    .range(120.0..=1400.0),
            )
            .column(Column::initial(96.0).at_least(70.0).clip(true))
            .column(Column::initial(150.0).at_least(90.0).clip(true))
            .column(Column::initial(150.0).at_least(90.0).clip(true))
            .column(Column::remainder().at_least(90.0).clip(true))
            .min_scrolled_height(0.0)
            .auto_shrink([false, false]);

        let table = table.header(24.0, |mut header| {
            header.col(|ui| {
                ui.label("");
            });
            for key in [
                SortKeySerde::Name,
                SortKeySerde::Size,
                SortKeySerde::Modified,
                SortKeySerde::Created,
                SortKeySerde::Type,
            ] {
                header.col(|ui| {
                    let arrow = if sort_key == key {
                        if sort_asc {
                            " ▲"
                        } else {
                            " ▼"
                        }
                    } else {
                        ""
                    };
                    let label = format!("{}{}", key.label(), arrow);
                    if ui
                        .add(egui::Button::new(RichText::new(label).strong()).frame(false))
                        .clicked()
                    {
                        clicked_cell.set(Some(key));
                    }
                });
            }
        });

        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        let mut drag_started: Option<PathBuf> = None;
        let mut drop_on: Option<PathBuf> = None;
        // The folder currently under the pointer during a drag (highlight only).
        let mut drag_hover: Option<PathBuf> = None;
        let pointer_released = ctx.input(|i| i.pointer.primary_released());

        table.body(|body| {
            body.rows(22.0, entries.len(), |mut row| {
                let idx = row.index();
                let entry = &entries[idx];
                let selected = self.current().is_selected(&entry.path);
                row.set_selected(selected);

                row.col(|ui| {
                    if let Some(id) = self.icon_for(ctx, entry, 16) {
                        ui.add(egui::Image::new((id, egui::vec2(16.0, 16.0))));
                    } else {
                        ui.label(if entry.is_dir { "🗀" } else { "🗎" });
                    }
                });

                row.col(|ui| {
                    let mut text = RichText::new(&entry.name);
                    if entry.hidden {
                        text = text.weak();
                    }
                    if entry.compressed {
                        text = text.color(Color32::from_rgb(120, 170, 255));
                    }
                    let resp = ui
                        .selectable_label(selected, text)
                        .interact(egui::Sense::click_and_drag());
                    if resp.drag_started() {
                        drag_started = Some(entry.path.clone());
                    }
                    // Hovering a folder only *highlights* it.  The drop is
                    // committed when the button comes up, which is what makes
                    // dragging feel right and stops a drag from "sticking": the
                    // old code armed the target on hover, so letting go anywhere
                    // could still drop into whatever folder happened to be under
                    // the pointer a moment earlier.
                    if entry.is_dir && resp.hovered() && self.drag_active {
                        drag_hover = Some(entry.path.clone());
                        ui.painter().rect_stroke(
                            resp.rect.expand(1.0),
                            2.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(0, 150, 255)),
                        );
                        if pointer_released {
                            drop_on = Some(entry.path.clone());
                        }
                    }
                    let mut local: Vec<Command> = Vec::new();
                    resp.context_menu(|ui| simple_context_menu(ui, entry, &app_list, &mut local));
                    for c in local {
                        commands.push((entry.path.clone(), c));
                    }
                    if resp.clicked() {
                        clicked = Some((idx, entry.path.clone()));
                    }
                    if resp.double_clicked() {
                        opened = Some(entry.clone());
                    }
                });
                row.col(|ui| {
                    let size = if entry.is_dir {
                        String::new()
                    } else {
                        disk::format_size(entry.size)
                    };
                    ui.label(RichText::new(size).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(entry.modified_display()).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(format_date(entry.created)).monospace());
                });
                row.col(|ui| {
                    let t = self.type_text(entry);
                    ui.label(RichText::new(t).weak());
                });
            });
        });

        if let Some(key) = clicked_cell.get() {
            self.set_sort(key);
        }
        if let Some(path) = drag_started {
            if !self.current().is_selected(&path) {
                self.current_mut().selection = vec![path.clone()];
            }
            self.drag_paths = self.current().selection.clone();
            self.drag_active = true;
        }
        if let Some(target) = drop_on {
            self.drop_target = Some(target);
        }
        if let Some((idx, path)) = clicked {
            let mods = ctx.input(|i| i.modifiers);
            self.apply_click(idx, &path, mods, entries);
            // Explorer also selects on right-click without clearing a multi-selection.
            if !self.current().is_selected(&path) {
                self.current_mut().selection = vec![path.clone()];
            }
        }
        if let Some(entry) = opened {
            self.open_entry(&entry);
        }
        self.finish_drag_and_commands(ctx, entries, commands);
    }

    pub(super) fn list_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entries: &[DirEntry],
    ) {
        let app_list = self.open_with.clone();
        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        let mut drag_started: Option<PathBuf> = None;
        let mut drop_on: Option<PathBuf> = None;
        // The folder currently under the pointer during a drag (highlight only).
        let mut drag_hover: Option<PathBuf> = None;
        let pointer_released = ctx.input(|i| i.pointer.primary_released());
        egui::ScrollArea::both()
            .id_salt("listview")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width().max(600.0);
                let cols = ((width / 230.0).floor() as usize).max(1);
                let cell_w = width / cols as f32 - 6.0;
                for chunk_start in (0..entries.len()).step_by(cols) {
                    let chunk = &entries[chunk_start..(chunk_start + cols).min(entries.len())];
                    ui.horizontal(|ui| {
                        for (offset, entry) in chunk.iter().enumerate() {
                            let idx = chunk_start + offset;
                            let selected = self.current().is_selected(&entry.path);
                            ui.allocate_ui(egui::vec2(cell_w, 20.0), |ui| {
                                ui.horizontal(|ui| {
                                    if let Some(id) = self.icon_for(ctx, entry, 16) {
                                        ui.add(egui::Image::new((id, egui::vec2(16.0, 16.0))));
                                    }
                                    let mut text =
                                        RichText::new(truncate_middle(&entry.name, 30)).size(12.0);
                                    if entry.hidden {
                                        text = text.weak();
                                    }
                                    let mut local: Vec<Command> = Vec::new();
                                    let resp = ui.add(
                                        egui::Button::new(text).selected(selected).frame(false),
                                    );
                                    resp.context_menu(|ui| {
                                        simple_context_menu(ui, entry, &app_list, &mut local)
                                    });
                                    commands
                                        .extend(local.into_iter().map(|c| (entry.path.clone(), c)));
                                    if resp.drag_started() {
                                        drag_started = Some(entry.path.clone());
                                    }
                                    if entry.is_dir && resp.hovered() && self.drag_active {
                                        drag_hover = Some(entry.path.clone());
                                        ui.painter().rect_stroke(
                                            resp.rect.expand(1.0),
                                            2.0,
                                            egui::Stroke::new(
                                                1.5_f32,
                                                Color32::from_rgb(0, 150, 255),
                                            ),
                                        );
                                        if pointer_released {
                                            drop_on = Some(entry.path.clone());
                                        }
                                    }
                                    if resp.clicked() {
                                        clicked = Some((idx, entry.path.clone()));
                                    }
                                    if resp.double_clicked() {
                                        opened = Some(entry.clone());
                                    }
                                });
                            });
                        }
                    });
                }
            });
        if let Some((idx, path)) = clicked {
            let mods = ctx.input(|i| i.modifiers);
            self.apply_click(idx, &path, mods, entries);
        }
        if let Some(path) = drag_started {
            if !self.current().is_selected(&path) {
                self.current_mut().selection = vec![path.clone()];
            }
            self.drag_paths = self.current().selection.clone();
            self.drag_active = true;
        }
        if let Some(target) = drop_on {
            self.drop_target = Some(target);
        }
        if let Some(entry) = opened {
            self.open_entry(&entry);
        }
        self.finish_drag_and_commands(ctx, entries, commands);
    }

    /// Large icons (Explorer's "Large icons" view) with 48 px shell icons.
    pub(super) fn tiles_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entries: &[DirEntry],
        icon: u32,
    ) {
        let app_list = self.open_with.clone();
        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        let mut drag_started: Option<PathBuf> = None;
        let mut drop_on: Option<PathBuf> = None;
        // The folder currently under the pointer during a drag (highlight only).
        let mut drag_hover: Option<PathBuf> = None;
        let pointer_released = ctx.input(|i| i.pointer.primary_released());
        // The background context menu's "Icon spacing" slider widens the tiles.
        // Explorer has no such control, the request asks for one, so the gap the
        // user picks is added to the fixed tile width.
        let gap = self.current().spacing.clamp(0.0, 180.0);
        let cell_w = 116.0 + gap;
        let cell_h = icon as f32 + 34.0 + gap * 0.35;
        egui::ScrollArea::vertical()
            .id_salt("tiles")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let avail = ui.available_width().max(320.0);
                let cols = ((avail / cell_w).floor() as usize).max(1);
                for chunk_start in (0..entries.len()).step_by(cols) {
                    let chunk = &entries[chunk_start..(chunk_start + cols).min(entries.len())];
                    ui.horizontal(|ui| {
                        for (offset, entry) in chunk.iter().enumerate() {
                            let idx = chunk_start + offset;
                            let selected = self.current().is_selected(&entry.path);
                            ui.allocate_ui(egui::vec2(cell_w, cell_h), |ui| {
                                ui.vertical_centered(|ui| {
                                    let id = self.icon_for(ctx, entry, icon);
                                    // `ImageButton`/`Button` only sense clicks, so a
                                    // folder tile could never start a drag - which
                                    // is why dragging "worked only for shortcuts".
                                    // An explicit `interact` with `click_and_drag`
                                    // gives every tile the same drag behaviour.
                                    if let Some(id) = id {
                                        ui.add(
                                            egui::ImageButton::new(egui::Image::new((
                                                id,
                                                egui::vec2(icon as f32, icon as f32),
                                            )))
                                            .selected(selected)
                                            .frame(false),
                                        );
                                    } else {
                                        let t = if entry.is_dir { "🗀" } else { "🗎" };
                                        ui.add(
                                            egui::Button::new(
                                                RichText::new(t).size(icon as f32 * 0.7),
                                            )
                                            .selected(selected)
                                            .frame(false),
                                        );
                                    }
                                    let tile_rect = egui::Rect::from_min_size(
                                        ui.cursor().min - egui::vec2(0.0, icon as f32 + 4.0),
                                        egui::vec2(icon as f32 + 8.0, icon as f32),
                                    );
                                    let resp = ui.interact(
                                        tile_rect,
                                        egui::Id::new(("tile_drag", idx)),
                                        egui::Sense::click_and_drag(),
                                    );
                                    let mut local: Vec<Command> = Vec::new();
                                    resp.context_menu(|ui| {
                                        simple_context_menu(ui, entry, &app_list, &mut local)
                                    });
                                    commands
                                        .extend(local.into_iter().map(|c| (entry.path.clone(), c)));
                                    if resp.drag_started() {
                                        drag_started = Some(entry.path.clone());
                                    }
                                    if entry.is_dir && resp.hovered() && self.drag_active {
                                        drag_hover = Some(entry.path.clone());
                                        ui.painter().rect_stroke(
                                            resp.rect.expand(1.0),
                                            2.0,
                                            egui::Stroke::new(
                                                1.5_f32,
                                                Color32::from_rgb(0, 150, 255),
                                            ),
                                        );
                                        if pointer_released {
                                            drop_on = Some(entry.path.clone());
                                        }
                                    }
                                    if resp.clicked() {
                                        clicked = Some((idx, entry.path.clone()));
                                    }
                                    if resp.double_clicked() {
                                        opened = Some(entry.clone());
                                    }
                                    let name =
                                        RichText::new(truncate_middle(&entry.name, 16)).size(11.0);
                                    let name = if entry.hidden { name.weak() } else { name };
                                    ui.label(name);
                                });
                            });
                        }
                    });
                }
            });
        if let Some((idx, path)) = clicked {
            let mods = ctx.input(|i| i.modifiers);
            self.apply_click(idx, &path, mods, entries);
        }
        if let Some(path) = drag_started {
            if !self.current().is_selected(&path) {
                self.current_mut().selection = vec![path.clone()];
            }
            self.drag_paths = self.current().selection.clone();
            self.drag_active = true;
        }
        if let Some(target) = drop_on {
            self.drop_target = Some(target);
        }
        if let Some(entry) = opened {
            self.open_entry(&entry);
        }
        self.finish_drag_and_commands(ctx, entries, commands);
    }

    /// Explorer's "Content" view: rows with a larger icon, name, and metadata.
    pub(super) fn content_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entries: &[DirEntry],
    ) {
        let app_list = self.open_with.clone();
        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, entry) in entries.iter().enumerate() {
                    let selected = self.current().is_selected(&entry.path);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 52.0),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter()
                            .rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
                    }
                    if resp.hovered() {
                        ui.painter().rect_stroke(
                            rect,
                            3.0,
                            egui::Stroke::new(
                                1.0_f32,
                                ui.visuals().widgets.hovered.bg_stroke.color,
                            ),
                        );
                    }
                    let icon_rect = egui::Rect::from_min_size(
                        rect.min + egui::vec2(6.0, 8.0),
                        egui::vec2(36.0, 36.0),
                    );
                    if let Some(id) = self.icon_for(ctx, entry, 32) {
                        ui.put(icon_rect, egui::Image::new((id, egui::vec2(36.0, 36.0))));
                    } else {
                        ui.put(
                            icon_rect,
                            egui::Label::new(if entry.is_dir { "🗀" } else { "🗎" }),
                        );
                    }
                    let name_pos = rect.min + egui::vec2(52.0, 8.0);
                    ui.painter().text(
                        name_pos,
                        egui::Align2::LEFT_TOP,
                        &entry.name,
                        egui::FontId::proportional(13.0),
                        if selected {
                            Color32::WHITE
                        } else {
                            ui.visuals().text_color()
                        },
                    );
                    let meta = if entry.is_dir {
                        String::new()
                    } else {
                        format!(
                            "{} · {}",
                            disk::format_size(entry.size),
                            entry.modified_display()
                        )
                    };
                    ui.painter().text(
                        name_pos + egui::vec2(0.0, 18.0),
                        egui::Align2::LEFT_TOP,
                        &meta,
                        egui::FontId::proportional(11.0),
                        Color32::from_gray(160),
                    );
                    let mut local: Vec<Command> = Vec::new();
                    resp.context_menu(|ui| simple_context_menu(ui, entry, &app_list, &mut local));
                    commands.extend(local.into_iter().map(|c| (entry.path.clone(), c)));
                    if resp.clicked() {
                        clicked = Some((idx, entry.path.clone()));
                    }
                    if resp.double_clicked() {
                        opened = Some(entry.clone());
                    }
                }
            });
        if let Some((idx, path)) = clicked {
            let mods = ctx.input(|i| i.modifiers);
            self.apply_click(idx, &path, mods, entries);
        }
        if let Some(entry) = opened {
            self.open_entry(&entry);
        }
        self.finish_drag_and_commands(ctx, entries, commands);
    }
}

/// Right-click menu built without touching `self`, so it can be used inside the
/// table/tile closures that already borrow the app.
pub(super) fn simple_context_menu(
    ui: &mut egui::Ui,
    entry: &DirEntry,
    app_list: &[winrust_core::fileassoc::OpenWith],
    out: &mut Vec<Command>,
) {
    if ui.button(t("Open")).clicked() {
        out.push(Command::Open);
        ui.close_menu();
    }
    if entry.is_dir {
        if ui.button(t("Open in new tab")).clicked() {
            out.push(Command::OpenNewTab);
            ui.close_menu();
        }
    } else {
        if ui.button(t("Edit")).clicked() {
            out.push(Command::Edit);
            ui.close_menu();
        }
        // "Open with": the programs this extension is actually registered
        // for, then the system chooser.  The list comes from the registry
        // (`HKCR\Applications\*\SupportedTypes`, `...\OpenWithList`), so it
        // shows whatever is installed instead of a fixed set.
        if app_list.is_empty() {
            if ui.button(t("Open with…")).clicked() {
                out.push(Command::OpenWith);
                ui.close_menu();
            }
        } else {
            ui.menu_button(t("Open with…"), |ui| {
                ui.set_min_width(220.0);
                let path = entry.path.clone();
                for app in app_list {
                    if ui.button(&app.name).clicked() {
                        out.push(Command::OpenWithProgram(app.command.clone(), path.clone()));
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button(t("Choose another app…")).clicked() {
                    out.push(Command::OpenWith);
                    ui.close_menu();
                }
            });
        }
        if ui.button(t("Run as administrator")).clicked() {
            out.push(Command::RunAsAdmin);
            ui.close_menu();
        }
        if matches!(
            entry.ext.as_str(),
            "zip" | "7z" | "rar" | "tar" | "gz" | "cab"
        ) && ui.button(t("Extract All…")).clicked()
        {
            out.push(Command::ExtractHere);
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button(t("Cut")).clicked() {
        out.push(Command::Cut);
        ui.close_menu();
    }
    if ui.button(t("Copy")).clicked() {
        out.push(Command::Copy);
        ui.close_menu();
    }
    if ui.button(t("Rename")).clicked() {
        out.push(Command::Rename);
        ui.close_menu();
    }
    if ui.button(t("Delete")).clicked() {
        out.push(Command::Delete);
        ui.close_menu();
    }
    if ui.button(t("Delete permanently")).clicked() {
        out.push(Command::DeletePermanent);
        ui.close_menu();
    }
    ui.separator();
    if ui.button(t("Copy as path")).clicked() {
        out.push(Command::CopyPath);
        ui.close_menu();
    }
    if ui.button(t("Properties")).clicked() {
        out.push(Command::Properties);
        ui.close_menu();
    }
}

/// Format a `FILETIME` (100 ns since 1601) as a local date.
pub(super) fn format_date(ft: u64) -> String {
    if ft == 0 {
        return String::new();
    }
    let unix = ft / 10_000_000u64;
    if unix < 11_644_473_600 {
        return String::new();
    }
    disk::format_local(unix - 11_644_473_600)
}

impl ExplorerApp {
    /// Runs after any view: applies queued context-menu commands, handles
    /// drag-and-drop between rows/folders and repaints while a job runs.
    pub(super) fn finish_drag_and_commands(
        &mut self,
        ctx: &egui::Context,
        _entries: &[DirEntry],
        commands: Vec<(PathBuf, Command)>,
    ) {
        // Context-menu commands (the path is the row they were invoked on).
        for (path, cmd) in commands {
            // Explorer acts on the whole selection when the clicked row is part
            // of it; otherwise it acts on just that row.
            if self.current().is_selected(&path) {
                self.run_selection(cmd);
            } else {
                self.current_mut().selection = vec![path.clone()];
                self.run_selection(cmd);
            }
        }

        // Drag & drop (driven by per-row responses, see `register_drop_zone`).
        if let Some(dest) = self.drop_target.take() {
            let src = self.drag_paths.clone();
            let into_self = src.iter().any(|p| p == &dest);
            if !src.is_empty() && !into_self {
                let mut jobs = Vec::new();
                for s in &src {
                    let target = disk::unique_destination(dest.join(file_name_of(s)));
                    jobs.push((s.clone(), target));
                }
                let tab = self.active;
                self.start_file_job(jobs, false, tab);
                self.status = format!("Copied {} item(s) → {}", src.len(), dest.display());
            }
            self.drag_paths.clear();
            self.drag_active = false;
        }
        // A drag ends when the button comes up, wherever the pointer is.  Without
        // this, releasing outside a folder left `drag_active` set for ever: the
        // ghost kept following the cursor and the next click was swallowed as a
        // drop, which is exactly the "перетаскивание залипает" the report saw.
        if ctx.input(|i| i.pointer.primary_released()) && self.drag_active {
            self.drag_active = false;
            self.drag_paths.clear();
            self.drag_ghost = None;
            self.drop_target = None;
        }
        if self.current().selection.is_empty() && self.drag_active {
            self.drag_active = false;
            self.drag_paths.clear();
        }
        if self.job.is_some() || self.current().loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}

//! Keyboard handling, the modal dialogs, and the window's own loop: what a
//! key does, what confirms a delete, and what happens on exit.

use super::commands::Command;
use super::state::{DragGhost, ExplorerApp};
use super::tabs::{file_name_of, new_file_in, Tab};
use super::viewmode::{save_state, PersistedState, SortKeySerde, ViewMode};
use eframe::egui;
use egui::{Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use winrust_core::i18n;
use winrust_core::shell;
use winrust_core::t;
use winrust_core::tf;
use winrust_core::ts;

// ---------------------------------------------------------------------------
// Drag ghost and mouse navigation
// ---------------------------------------------------------------------------

impl ExplorerApp {
    fn drag_ghost(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        if self.drag_active && !self.drag_paths.is_empty() {
            let count = self.drag_paths.len();
            let label = self
                .drag_paths
                .first()
                .map(|p| file_name_of(p.as_path()))
                .unwrap_or_default();
            let short = i18n::middle(&label, 28, 14);
            match &mut self.drag_ghost {
                Some(g) if g.count == count && g.label == label => {
                    // Ease: 45 % of the remaining distance per frame at 60 fps.
                    if let Some(p) = pointer {
                        let dt = ctx.input(|i| i.stable_dt).clamp(0.004, 0.05);
                        let k = 1.0 - (-dt * 22.0).exp();
                        g.pos = g.pos + (p - g.pos) * k;
                    }
                }
                _ => {
                    self.drag_ghost = Some(DragGhost {
                        label,
                        short,
                        count,
                        pos: pointer.unwrap_or(egui::Pos2::ZERO),
                        spawned: std::time::Instant::now(),
                    });
                }
            }
            // Keep animating while the drag is live.
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else if self.drag_ghost.is_some() {
            // Drop: the ghost disappears; a short repaint settles the fade.
            self.drag_ghost = None;
        }

        let Some(ghost) = self.drag_ghost.clone() else {
            return;
        };
        if !self.drag_active {
            return;
        }
        let age = ghost.spawned.elapsed().as_secs_f32();
        let scale = (age / 0.12).clamp(0.35, 1.0);
        let text = if ghost.count > 1 {
            tf!("{} items", ghost.count)
        } else {
            ghost.label.clone()
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("drag_ghost"),
        ));
        let pal = winrust_ui::theme::pal(&self.appearance);

        // The translucent picture above the cursor: Explorer's drag image.  A
        // folder or file gets its own shell icon, so the picture matches what is
        // being dragged; the card below carries the name.
        let icon_side = 40.0 * scale.max(0.55);
        let icon_center = ghost.pos - egui::vec2(0.0, 22.0 * scale);
        let icon_rect = egui::Rect::from_center_size(icon_center, egui::vec2(icon_side, icon_side));
        // `icon_for_path` takes `&mut self`, so the path has to leave the
        // borrow of `drag_paths` first - the closure would otherwise hold a
        // shared borrow of the same field the call needs exclusively.
        let dragged = self.drag_paths.first().cloned();
        let tex = dragged
            .as_ref()
            .and_then(|p| self.icon_for_path(ctx, p, 48));
        match tex {
            Some(id) => {
                painter.image(
                    id,
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::from_white_alpha(150),
                );
            }
            None => {
                painter.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "📄",
                    egui::FontId::proportional(icon_side * 0.8),
                    Color32::from_white_alpha(150),
                );
            }
        }

        // Settings > drag image: the minimal style is the translucent icon
        // alone, the default is the icon plus the name card below.
        if self.minimal_drag_image {
            return;
        }
        let size = egui::vec2(190.0, 26.0) * scale;
        let rect = egui::Rect::from_min_size(ghost.pos + egui::vec2(14.0, 12.0), size);
        painter.rect_filled(
            rect,
            5.0,
            Color32::from_rgba_unmultiplied(pal.selection.0, pal.selection.1, pal.selection.2, 200),
        );
        painter.rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2),
            ),
        );
        let label = if ghost.count > 1 {
            text.clone()
        } else {
            ghost.short.clone()
        };
        painter.text(
            rect.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(12.0 * scale.max(0.6)),
            Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
        );
        // A count badge when several items travel together.
        if ghost.count > 1 {
            let badge = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(14.0, 0.0),
                egui::vec2(22.0, 16.0),
            );
            painter.rect_filled(
                badge,
                8.0,
                Color32::from_rgb(pal.accent.0, pal.accent.1, pal.accent.2),
            );
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                ghost.count.to_string(),
                egui::FontId::proportional(11.0),
                Color32::from_rgb(16, 22, 30),
            );
        }
    }

    /// Mouse side buttons: XButton1 = Back, XButton2 = Forward, as in Explorer.
    fn mouse_navigation(&mut self, ctx: &egui::Context) {
        let (back, forward) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if back {
            self.go_back();
        }
        if forward {
            self.go_forward();
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

impl ExplorerApp {
    fn keyboard(&mut self, ctx: &egui::Context) {
        // Ignore shortcuts while a text field has focus (rename / new folder /
        // address bar), exactly like Explorer.
        let typing = ctx.memory(|m| m.focused().is_some());
        let (keys, mods) = ctx.input(|i| {
            let mut v: Vec<egui::Key> = Vec::new();
            for k in [
                egui::Key::F5,
                egui::Key::F2,
                egui::Key::Delete,
                egui::Key::Backspace,
                egui::Key::Enter,
                egui::Key::Escape,
                egui::Key::A,
                egui::Key::C,
                egui::Key::X,
                egui::Key::V,
                egui::Key::T,
                egui::Key::W,
                egui::Key::L,
                egui::Key::N,
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::Home,
                egui::Key::End,
                egui::Key::Tab,
            ] {
                if i.key_pressed(k) {
                    v.push(k);
                }
            }
            (v, i.modifiers)
        });
        if typing && !mods.ctrl {
            return;
        }
        let has = |k: egui::Key| keys.contains(&k);
        let ctrl = mods.ctrl;
        let shift = mods.shift;
        let alt = mods.alt;
        let _ = ctrl;

        if has(egui::Key::F5) {
            self.refresh();
        }
        if has(egui::Key::Escape) {
            self.search_results = None;
            self.current_mut().filter.clear();
            self.current_mut().selection.clear();
        }
        if has(egui::Key::Enter) {
            if let Some(path) = self.current().selected_one().cloned() {
                if path.is_dir() {
                    self.navigate(path);
                } else {
                    let _ = shell::open(&path);
                }
            }
        }
        if has(egui::Key::Backspace) && !ctrl {
            self.go_up();
        }
        if mods.alt && has(egui::Key::ArrowUp) {
            self.go_up();
        }
        if mods.alt && has(egui::Key::ArrowDown) {
            // Alt+↓ on a folder opens it (Explorer's dropdown equivalent).
            if let Some(path) = self.current().selected_one().cloned() {
                if path.is_dir() {
                    self.navigate(path);
                }
            }
        }
        if alt && has(egui::Key::ArrowUp) {
            self.go_up();
        }
        if ctrl && has(egui::Key::T) {
            let p = self.current().path.clone();
            self.tabs.push(Tab::new(p));
            self.active = self.tabs.len() - 1;
            self.start_load(self.active);
        }
        if ctrl && has(egui::Key::W) && self.tabs.len() > 1 {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len() - 1);
            self.start_load(self.active);
        }
        if ctrl && has(egui::Key::L) {
            let p = self.current().path.display().to_string();
            self.current_mut().address_edit = Some(p);
        }
        if ctrl && shift && has(egui::Key::N) {
            self.creating_folder = true;
            self.new_folder_buffer = t("New folder");
        }
        if ctrl && has(egui::Key::A) {
            self.run_selection(Command::SelectAll);
        }
        if has(egui::Key::F2) {
            if let Some(one) = self.current().selected_one().cloned() {
                self.rename_buffer = file_name_of(&one);
                self.rename_target = Some(one);
            }
        }
        if has(egui::Key::Delete) {
            if shift {
                self.run_selection(Command::DeletePermanent);
            } else {
                self.run_selection(Command::Delete);
            }
        }
        if ctrl && has(egui::Key::C) {
            self.run_selection(Command::Copy);
        }
        if ctrl && has(egui::Key::X) {
            self.run_selection(Command::Cut);
        }
        if ctrl && has(egui::Key::V) {
            self.paste_clipboard();
        }
        // Arrow navigation moves the selection through the sorted list.
        if has(egui::Key::ArrowDown) || has(egui::Key::ArrowUp) {
            let entries = self.sorted_filtered();
            if !entries.is_empty() {
                let cur = self
                    .current()
                    .selection
                    .first()
                    .and_then(|p| entries.iter().position(|e| e.path == *p));
                let next = match (cur, has(egui::Key::ArrowDown)) {
                    (Some(i), true) => (i + 1).min(entries.len() - 1),
                    (Some(i), false) => i.saturating_sub(1),
                    (None, true) => 0,
                    (None, false) => entries.len() - 1,
                };
                if shift {
                    let mods = egui::Modifiers {
                        shift: true,
                        ..Default::default()
                    };
                    self.apply_click(next, &entries[next].path.clone(), mods, &entries);
                } else {
                    self.current_mut().selection = vec![entries[next].path.clone()];
                    self.current_mut().anchor = Some(next);
                }
            }
        }
        if has(egui::Key::Home) && ctrl {
            self.current_mut().selection.clear();
        }
    }

    // ------------------------------------------------------------------
    // Modals
    // ------------------------------------------------------------------

    fn modals(&mut self, ctx: &egui::Context) {
        if !self.delete_confirm.is_empty() {
            let paths = self.delete_confirm.clone();
            let permanent = self.delete_permanent;
            let mut action: Option<bool> = None;
            egui::Window::new(if permanent {
                "Delete permanently"
            } else {
                "Delete"
            })
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(if paths.len() == 1 {
                    if permanent {
                        format!(
                            "Permanently delete \"{}\"? This cannot be undone.",
                            file_name_of(&paths[0])
                        )
                    } else {
                        format!("Move \"{}\" to the Recycle Bin?", file_name_of(&paths[0]))
                    }
                } else if permanent {
                    format!(
                        "Permanently delete {} items? This cannot be undone.",
                        paths.len()
                    )
                } else {
                    format!("Move {} items to the Recycle Bin?", paths.len())
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(if permanent {
                            "Delete permanently"
                        } else {
                            "Delete"
                        })
                        .clicked()
                    {
                        action = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(false);
                    }
                });
            });
            match action {
                Some(true) => {
                    let report = winrust_core::files::delete(&paths, permanent);
                    self.status = if report.ok() {
                        if permanent {
                            format!("Deleted {} item(s)", report.done)
                        } else {
                            format!("Moved {} item(s) to the Recycle Bin", report.done)
                        }
                    } else {
                        format!(
                            "{} error(s): {}",
                            report.errors.len(),
                            report.errors.join("; ")
                        )
                    };
                    self.delete_confirm.clear();
                    self.refresh();
                }
                Some(false) => self.delete_confirm.clear(),
                None => {}
            }
        }

        if let Some(job) = &self.job {
            let fraction = if job.total == 0 {
                0.0
            } else {
                job.done as f32 / job.total as f32
            };
            let label = job.label.clone();
            let done = job.done;
            let total = job.total;
            egui::Window::new("Working…")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(label);
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .show_percentage()
                            .desired_width(320.0),
                    );
                    ui.label(format!("{done} / {total}"));
                    if ui.button("Cancel").clicked() {
                        if let Some(job) = &self.job {
                            job.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
        }
    }
}

impl eframe::App for ExplorerApp {
    /// The window's clear colour: the theme background with the configured
    /// opacity, so transparency reaches the compositor in every program.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        winrust_ui::theme::bg_color(&self.appearance)
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.pump_workers(ctx);
        self.keyboard(ctx);
        self.mouse_navigation(ctx);
        if self.hwnd == 0 {
            self.hwnd = winrust_ui::theme::hwnd_from_handle(frame).unwrap_or(0);
            if self.appearance.transparent {
                self.appearance_dirty = true;
            }
        }
        if self.appearance_dirty {
            self.appearance = self.appearance.sanitized();
            winrust_ui::theme::apply(ctx, &self.appearance);
            winrust_ui::theme::apply_window(self.hwnd, &self.appearance);
            let _ = i18n::settings::save(&self.appearance);
            self.appearance_dirty = false;
        }
        // The launcher owns the language switch; pick up a change it made so a
        // running Explorer window follows along instead of needing a restart.
        // Polled once a second — a registry read per frame would be wasteful.
        if self.lang_checked_at.elapsed() >= std::time::Duration::from_secs(1) {
            self.lang_checked_at = std::time::Instant::now();
            let stored_lang = i18n::lang::load();
            if stored_lang != self.lang {
                self.lang = stored_lang;
                i18n::set_lang(stored_lang);
                winrust_ui::theme::apply(ctx, &self.appearance);
            }
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_bar(ui));
        egui::TopBottomPanel::top("commandbar").show(ctx, |ui| self.command_bar(ui));
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .show(ctx, |ui| self.status_bar(ui));
        if self.preview_open {
            egui::SidePanel::right("preview")
                .resizable(true)
                .default_width(280.0)
                .width_range(180.0..=520.0)
                .show(ctx, |ui| self.preview_pane(ui, ctx));
        }
        let sidebar_width = self.sidebar_width;
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(sidebar_width)
            .width_range(180.0..=380.0)
            .show(ctx, |ui| self.sidebar(ui, ctx));
        egui::CentralPanel::default().show(ctx, |ui| {
            let bg = ui.interact(
                ui.available_rect_before_wrap(),
                egui::Id::new("explorer_background"),
                egui::Sense::click(),
            );
            let mut cmds: Vec<Command> = Vec::new();
            // The menu closure borrows `self`, so everything it needs from the
            // registry is fetched *before* it runs, and only when it is stale.
            let here = self.current().path.clone();
            let new_stale = self
                .new_items_at
                .map(|t| t.elapsed() > std::time::Duration::from_secs(5))
                .unwrap_or(true);
            if new_stale && bg.secondary_clicked() {
                self.new_items = winrust_core::fileassoc::new_items();
                self.new_items_at = Some(std::time::Instant::now());
            }
            let new_items = self.new_items.clone();
            bg.context_menu(|ui| {
                ui.set_min_width(230.0);
                // View: the three tile sizes and the grid switches, exactly the
                // order Windows 11 uses.
                ui.menu_button(t("View"), |ui| {
                    ui.set_min_width(220.0);
                    // Icon sizes are their own axis in Explorer: they select the
                    // tiles view *and* the picture size.
                    for (px, label) in [
                        (48u32, "Large icons"),
                        (32u32, "Medium icons"),
                        (24u32, "Small icons"),
                    ] {
                        let selected =
                            self.current().view == ViewMode::Tiles && self.current().icon_px == px;
                        if ui.selectable_label(selected, t(label)).clicked() {
                            let tab = self.current_mut();
                            tab.view = ViewMode::Tiles;
                            tab.icon_px = px;
                            self.remember_prefs();
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    for (view, label) in [
                        (ViewMode::List, "List"),
                        (ViewMode::Details, "Details"),
                        (ViewMode::Content, "Content"),
                    ] {
                        let selected = self.current().view == view;
                        if ui.selectable_label(selected, t(label)).clicked() {
                            self.current_mut().view = view;
                            self.remember_prefs();
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button(t("Sort by"), |ui| {
                    ui.set_min_width(220.0);
                    let current = self.current().sort_key;
                    for key in SortKeySerde::ALL {
                        if ui.selectable_label(current == key, key.label()).clicked() {
                            self.set_sort(key);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let asc = self.current().sort_asc;
                    if ui.selectable_label(asc, t("Ascending")).clicked() {
                        self.current_mut().sort_asc = true;
                        self.remember_prefs();
                        self.refresh();
                        ui.close_menu();
                    }
                    if ui.selectable_label(!asc, t("Descending")).clicked() {
                        self.current_mut().sort_asc = false;
                        self.remember_prefs();
                        self.refresh();
                        ui.close_menu();
                    }
                });
                // Drag image: the second variant from the screenshot, a
                // setting rather than a hard choice.  Windows calls it
                // "Show drag image" and lets the user drop the name card.
                if ui
                    .selectable_label(self.minimal_drag_image, t("Icon-only drag image"))
                    .clicked()
                {
                    self.minimal_drag_image = !self.minimal_drag_image;
                    ui.close_menu();
                }
                ui.separator();
                // Tile spacing: Explorer never let the user set this, but the
                // request explicitly asks for it, so it lives here as a slider.
                ui.menu_button(t("Icon spacing"), |ui| {
                    ui.set_min_width(240.0);
                    let mut gap = self.current().spacing;
                    if ui
                        .add(egui::Slider::new(&mut gap, 0.0..=180.0).text(t("pixels")))
                        .changed()
                    {
                        self.current_mut().spacing = gap;
                        self.remember_prefs();
                    }
                    ui.label(
                        RichText::new(ts("Applies to the icon views of this folder"))
                            .small()
                            .weak(),
                    );
                });
                if ui.button(t("Refresh")).clicked() {
                    cmds.push(Command::Refresh);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Paste")).clicked() {
                    cmds.push(Command::Paste);
                    ui.close_menu();
                }
                ui.menu_button(t("New"), |ui| {
                    ui.set_min_width(240.0);
                    if ui.button(t("Folder")).clicked() {
                        self.creating_folder = true;
                        self.new_folder_buffer = t("New folder");
                        ui.close_menu();
                    }
                    if ui.button(t("Shortcut")).clicked() {
                        let _ = shell::launch("rundll32.exe", "appwiz.cpl,NewLinkHere %1");
                        ui.close_menu();
                    }
                    ui.separator();
                    // Everything else is what this machine has actually
                    // registered: Word, Excel, WinRAR, SAP ... appear only when
                    // they are installed, exactly like the real submenu.
                    //
                    // The list is read from the registry when the menu opens
                    // (above), never once per frame.
                    for item in new_items.iter() {
                        if ui.button(&item.label).clicked() {
                            new_file_in(&here, &item.extension);
                            cmds.push(Command::Refresh);
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if ui.button(t("Select all")).clicked() {
                    cmds.push(Command::SelectAll);
                    ui.close_menu();
                }
                if ui.button(t("Invert selection")).clicked() {
                    cmds.push(Command::InvertSelection);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Send to Desktop")).clicked() {
                    cmds.push(Command::SendToDesktop);
                    ui.close_menu();
                }
                if ui.button(t("Properties")).clicked() {
                    let _ = winrust_core::shell::show_properties(&self.current().path);
                    ui.close_menu();
                }
            });
            if bg.clicked() {
                self.current_mut().selection.clear();
                self.current_mut().anchor = None;
            }
            for c in cmds {
                // Match by reference: `Command` is not `Copy` any more, so
                // `matches!(c, ...)` would move it before `run` gets it.
                if matches!(&c, Command::SendToDesktop) {
                    self.run_selection(c);
                } else {
                    self.run(PathBuf::new(), c);
                }
            }
            self.file_area(ui, ctx);
        });

        self.modals(ctx);
        // Drawn last so the drag ghost floats above every panel.
        self.drag_ghost(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember_prefs();
        let state = PersistedState {
            tabs: self.tabs.iter().map(|t| t.path.clone()).collect(),
            active_tab: self.active,
            folders: self.folders.clone(),
            preview_open: self.preview_open,
            sidebar_width: self.sidebar_width,
            pinned: self.pinned.clone(),
            minimal_drag_image: self.minimal_drag_image,
        };
        save_state(&state);
    }
}

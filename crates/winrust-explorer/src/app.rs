//! Explorer window: sidebar + tabs + views + context menu + file operations.
//!
//! State model
//! -----------
//! * `ExplorerApp` owns any number of `Tab`s (Ctrl+T).
//! * Each tab has an independent history, sort and view mode.
//! * Directory listings happen on a worker thread so a slow network share can
//!   never freeze the window; results arrive through a channel.
//! * Per-folder preferences (view mode, show hidden) are persisted to
//!   `%APPDATA%\WinRust\explorer.ini` on exit and between sessions.

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use winrust_core::disk::{self, DirEntry, SortKey};
use winrust_core::shell::{self, KnownFolder};
use winrust_core::user;

/// Which layout the file area uses (Explorer's View menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ViewMode {
    Details,
    List,
    Tiles,
    SmallIcons,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            ViewMode::Details => "Details",
            ViewMode::List => "List",
            ViewMode::Tiles => "Tiles",
            ViewMode::SmallIcons => "Small icons",
        }
    }
    const ALL: [ViewMode; 4] = [ViewMode::Details, ViewMode::List, ViewMode::Tiles, ViewMode::SmallIcons];
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FolderPrefs {
    view: ViewMode,
    show_hidden: bool,
    sort_key: SortKeySerde,
    sort_ascending: bool,
}

impl Default for FolderPrefs {
    fn default() -> Self {
        Self {
            view: ViewMode::Details,
            show_hidden: false,
            sort_key: SortKeySerde::Name,
            sort_ascending: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum SortKeySerde {
    Name,
    Size,
    Modified,
    Type,
}

impl SortKeySerde {
    fn to_core(self) -> SortKey {
        match self {
            SortKeySerde::Name => SortKey::Name,
            SortKeySerde::Size => SortKey::Size,
            SortKeySerde::Modified => SortKey::Modified,
            SortKeySerde::Type => SortKey::Type,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PersistedState {
    tabs: Vec<PathBuf>,
    active_tab: usize,
    folders: HashMap<String, FolderPrefs>,
}

fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(base).join("WinRust");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("explorer.json"))
}

fn load_state() -> PersistedState {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(state: &PersistedState) {
    if let Some(p) = state_path() {
        if let Ok(s) = serde_json::to_string_pretty(state) {
            let _ = std::fs::write(p, s);
        }
    }
}

/// One browser tab.
struct Tab {
    path: PathBuf,
    history: Vec<PathBuf>,
    history_pos: usize,
    entries: Vec<DirEntry>,
    loading: bool,
    error: Option<String>,
    selected: Option<PathBuf>,
    /// Inline filter typed into the address bar.
    filter: String,
    address_edit: Option<String>,
    view: ViewMode,
    show_hidden: bool,
    sort_key: SortKeySerde,
    sort_asc: bool,
    request_id: u64,
}

impl Tab {
    fn new(path: PathBuf) -> Self {
        Self {
            path: path.clone(),
            history: vec![path],
            history_pos: 0,
            entries: Vec::new(),
            loading: false,
            error: None,
            selected: None,
            filter: String::new(),
            address_edit: None,
            view: ViewMode::Details,
            show_hidden: false,
            sort_key: SortKeySerde::Name,
            sort_asc: true,
            request_id: 0,
        }
    }

    fn title(&self) -> String {
        file_name_of(&self.path)
    }

    fn can_back(&self) -> bool {
        self.history_pos > 0
    }

    fn can_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }
}

fn file_name_of(p: &Path) -> String {
    match p.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() { "This PC".into() } else { s }
        }
    }
}

/// Message from the directory worker.
struct Listing {
    tab_index: usize,
    request_id: u64,
    result: Result<Vec<DirEntry>, String>,
}

pub struct ExplorerApp {
    tabs: Vec<Tab>,
    active: usize,
    rx: Receiver<Listing>,
    tx: Sender<Listing>,
    next_request: u64,
    folders: HashMap<String, FolderPrefs>,
    user: user::UserInfo,
    status: String,
    icons: HashMap<String, IconTexture>,
    nav_selected: Option<KnownFolder>,
    drives: Vec<disk::VolumeInfo>,
    last_refresh: std::time::Instant,
    rename_target: Option<PathBuf>,
    rename_buffer: String,
    creating_folder: bool,
    new_folder_buffer: String,
    delete_confirm: Vec<PathBuf>,
}

/// Cached egui texture for one icon key.
struct IconTexture {
    texture: Option<egui::TextureHandle>,
}

impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let persisted = load_state();
        let default_start = KnownFolder::Profile.path().unwrap_or_else(|| PathBuf::from("C:\\"));
        let tabs: Vec<Tab> = if persisted.tabs.is_empty() {
            vec![Tab::new(default_start)]
        } else {
            persisted
                .tabs
                .iter()
                .map(|p| {
                    let mut t = Tab::new(p.clone());
                    if let Some(pref) = persisted.folders.get(&p.to_string_lossy().to_string()) {
                        t.view = pref.view;
                        t.show_hidden = pref.show_hidden;
                        t.sort_key = pref.sort_key;
                        t.sort_asc = pref.sort_ascending;
                    }
                    t
                })
                .collect()
        };
        let active = persisted.active_tab.min(tabs.len().saturating_sub(1));
        let (tx, rx) = channel();
        let mut app = Self {
            tabs,
            active,
            rx,
            tx,
            next_request: 1,
            folders: persisted.folders,
            user: user::current_user(),
            status: String::new(),
            icons: HashMap::new(),
            nav_selected: Some(KnownFolder::Profile),
            drives: disk::volumes(),
            last_refresh: std::time::Instant::now(),
            rename_target: None,
            rename_buffer: String::new(),
            creating_folder: false,
            new_folder_buffer: String::new(),
            delete_confirm: Vec::new(),
        };
        app.start_load(app.active);
        app
    }

    fn current(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Kick off an async directory read for a tab.
    fn start_load(&mut self, tab_index: usize) {
        let (path, show_hidden) = {
            let tab = &self.tabs[tab_index];
            (tab.path.clone(), tab.show_hidden)
        };
        self.next_request += 1;
        let request_id = self.next_request;
        self.tabs[tab_index].request_id = request_id;
        self.tabs[tab_index].loading = true;
        self.tabs[tab_index].error = None;

        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = disk::read_dir(&path, show_hidden).map_err(|e| e.to_string());
            let _ = tx.send(Listing { tab_index, request_id, result });
        });
    }

    fn navigate(&mut self, path: PathBuf) {
        let prefs = self.folders.get(&path.to_string_lossy().to_string()).cloned();
        {
            let tab = self.current_mut();
            tab.history.truncate(tab.history_pos + 1);
            tab.history.push(path.clone());
            tab.history_pos = tab.history.len() - 1;
            tab.path = path.clone();
            tab.selected = None;
            tab.filter.clear();
            if let Some(p) = prefs {
                tab.view = p.view;
                tab.show_hidden = p.show_hidden;
                tab.sort_key = p.sort_key;
                tab.sort_asc = p.sort_ascending;
            }
        }
        self.remember_prefs();
        self.start_load(self.active);
    }

    fn go_back(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos > 0 {
            tab.history_pos -= 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selected = None;
        }
        self.start_load(self.active);
    }

    fn go_forward(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos + 1 < tab.history.len() {
            tab.history_pos += 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selected = None;
        }
        self.start_load(self.active);
    }

    fn go_up(&mut self) {
        let parent = self.current().path.parent().map(|p| p.to_path_buf());
        if let Some(p) = parent {
            self.navigate(p);
        }
    }

    fn remember_prefs(&mut self) {
        for tab in &self.tabs {
            self.folders.insert(
                tab.path.to_string_lossy().to_string(),
                FolderPrefs {
                    view: tab.view,
                    show_hidden: tab.show_hidden,
                    sort_key: tab.sort_key,
                    sort_ascending: tab.sort_asc,
                },
            );
        }
    }

    fn refresh(&mut self) {
        self.start_load(self.active);
    }

    fn set_sort(&mut self, key: SortKeySerde) {
        let tab = self.current_mut();
        if tab.sort_key == key {
            tab.sort_asc = !tab.sort_asc;
        } else {
            tab.sort_key = key;
            tab.sort_asc = true;
        }
        self.remember_prefs();
    }

    fn sorted_filtered(&self) -> Vec<DirEntry> {
        let tab = self.current();
        let mut v: Vec<DirEntry> = tab
            .entries
            .iter()
            .filter(|e| {
                tab.filter.is_empty() || e.name.to_lowercase().contains(&tab.filter.to_lowercase())
            })
            .cloned()
            .collect();
        disk::sort_by(&mut v, tab.sort_key.to_core(), tab.sort_asc);
        v
    }

    fn open_entry(&mut self, entry: &DirEntry) {
        if entry.is_dir {
            self.navigate(entry.path.clone());
        } else if let Err(e) = shell::open(&entry.path) {
            self.status = format!("Cannot open {}: {e}", entry.name);
        }
    }

    fn icon_for(&mut self, ctx: &egui::Context, key: &str, fetch: impl FnOnce() -> Option<Vec<u8>>) -> Option<egui::TextureId> {
        if let Some(entry) = self.icons.get(key) {
            return entry.texture.as_ref().map(|t| t.id());
        }
        let rgba = fetch();
        let texture = rgba.map(|rgba| {
            // egui wants RGBA; winrust-core already produces that.
            let image = egui::ColorImage::from_rgba_unmultiplied([16, 16], &rgba);
            ctx.load_texture(key.to_string(), image, egui::TextureOptions::LINEAR)
        });
        let id = texture.as_ref().map(|t| t.id());
        self.icons.insert(key.to_string(), IconTexture { texture });
        id
    }

    // ------------------------------------------------------------------
    // Panels
    // ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.add_enabled(self.current().can_back(), egui::Button::new("◀")).on_hover_text("Back").clicked() {
                self.go_back();
            }
            if ui.add_enabled(self.current().can_forward(), egui::Button::new("▶")).on_hover_text("Forward").clicked() {
                self.go_forward();
            }
            if ui.button("▲").on_hover_text("Up one level").clicked() {
                self.go_up();
            }
            if ui.button("⟳").on_hover_text("Refresh (F5)").clicked() {
                self.refresh();
            }

            // Breadcrumb / address edit
            let editing = self.current().address_edit.is_some();
            if editing {
                let mut buf = self.current().address_edit.clone().unwrap_or_default();
                let response = ui.add_sized(
                    [ui.available_width() - 90.0, 22.0],
                    egui::TextEdit::singleline(&mut buf).hint_text("Type a path and press Enter"),
                );
                self.current_mut().address_edit = Some(buf.clone());
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.current_mut().address_edit = None;
                    let p = PathBuf::from(buf);
                    if p.exists() {
                        self.navigate(p);
                    } else {
                        self.status = format!("Path not found: {}", p.display());
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.current_mut().address_edit = None;
                }
            } else {
                let mut target: Option<PathBuf> = None;
                egui::ScrollArea::horizontal().id_salt("breadcrumb").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let path = self.current().path.clone();
                        let mut acc = PathBuf::new();
                        ui.label(RichText::new("🗀").size(15.0));
                        for (idx, comp) in path.components().enumerate() {
                            acc.push(comp);
                            let label = comp.as_os_str().to_string_lossy().to_string();
                            let text = if label.is_empty() {
                                path.display().to_string()
                            } else if idx == 0 {
                                // Drive letters already read well on their own.
                                label
                            } else {
                                label
                            };
                            if ui.link(text).clicked() {
                                target = Some(acc.clone());
                            }
                            ui.label("›");
                        }
                        if ui.small_button("✎").on_hover_text("Type a path").clicked() {
                            let p = self.current().path.display().to_string();
                            self.current_mut().address_edit = Some(p);
                        }
                    });
                });
                if let Some(t) = target {
                    self.navigate(t);
                }
            }

            ui.separator();
            // Search filter
            let mut filter = self.current().filter.clone();
            let search = ui.add_sized(
                [220.0, 22.0],
                egui::TextEdit::singleline(&mut filter).hint_text("🔍 Search this folder"),
            );
            if search.changed() {
                self.current_mut().filter = filter;
            }
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(6.0);
        ui.heading(RichText::new("WinRust Explorer").size(16.0));
        ui.add_space(4.0);
        ui.separator();

        let mut nav: Option<PathBuf> = None;

        ui.add_space(4.0);
        ui.label(RichText::new("QUICK ACCESS").small().weak());
        for folder in KnownFolder::sidebar() {
            let label = folder.label();
            let selected = self.nav_selected == Some(*folder);
            if ui.selectable_label(selected, format!("  {label}")).clicked() {
                self.nav_selected = Some(*folder);
                if let Some(p) = folder.path() {
                    nav = Some(p);
                } else {
                    self.status = format!("{label} is a virtual folder");
                }
            }
        }

        ui.add_space(10.0);
        ui.label(RichText::new("DRIVES").small().weak());
        for drive in &self.drives {
            let p = PathBuf::from(&drive.root);
            let name = if drive.label.is_empty() {
                format!("{} {}", drive.root.trim_end_matches('\\'), drive.kind.map(|k| k.label()).unwrap_or(""))
            } else {
                format!("{} ({})", drive.label, drive.root.trim_end_matches('\\'))
            };
            let frac = if drive.total_bytes > 0 {
                drive.free_bytes as f32 / drive.total_bytes as f32
            } else {
                0.0
            };
            let resp = ui.add(egui::Button::new(RichText::new(format!("  💽 {name}")).size(13.0)).frame(false));
            ui.add(
                egui::ProgressBar::new(1.0 - frac)
                    .desired_height(4.0)
                    .desired_width(ui.available_width() - 8.0),
            );
            if resp.clicked() {
                nav = Some(p);
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(4.0);
            let who = if self.user.account.is_empty() {
                "user".to_string()
            } else {
                format!("{}\\{}", self.user.domain, self.user.account)
            };
            ui.label(
                RichText::new(format!(
                    "{}  ·  {}  ·  {}",
                    who,
                    self.user.integrity,
                    if self.user.elevated { "admin" } else { "standard" }
                ))
                .small()
                .weak(),
            );
            if !self.user.elevated {
                ui.label(RichText::new("Tip: run elevated for protected folders").small().weak());
            }
        });

        if let Some(p) = nav {
            self.navigate(p);
        }
        let _ = ctx;
    }

    fn view_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // View / sort toolbar
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}", self.current().path.display())).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut view = self.current().view;
                egui::ComboBox::from_id_salt("viewmode")
                    .selected_text(view.label())
                    .show_ui(ui, |ui| {
                        for v in ViewMode::ALL {
                            if ui.selectable_value(&mut view, v, v.label()).clicked() {
                                self.current_mut().view = v;
                                self.remember_prefs();
                            }
                        }
                    });
                let mut hidden = self.current().show_hidden;
                if ui.checkbox(&mut hidden, "Hidden").changed() {
                    self.current_mut().show_hidden = hidden;
                    self.remember_prefs();
                    self.refresh();
                }
                if ui.button("New folder").on_hover_text("Ctrl+Shift+N").clicked() {
                    self.creating_folder = true;
                    self.new_folder_buffer = "New folder".into();
                }
            });
        });
        ui.separator();

        if let Some(err) = self.current().error.clone() {
            ui.colored_label(Color32::from_rgb(240, 120, 120), format!("⚠ {err}"));
            if ui.button("Retry").clicked() {
                self.refresh();
            }
            return;
        }

        let entries = self.sorted_filtered();
        let view = self.current().view;

        // Pending new-folder inline editor
        if self.creating_folder {
            ui.horizontal(|ui| {
                ui.label("New folder name:");
                ui.text_edit_singleline(&mut self.new_folder_buffer);
                if ui.button("Create").clicked() {
                    let target = self.current().path.join(&self.new_folder_buffer);
                    match disk::create_dir_all(&target) {
                        Ok(()) => {
                            self.status = format!("Created {}", target.display());
                            self.creating_folder = false;
                            self.refresh();
                        }
                        Err(e) => self.status = format!("Create failed: {e}"),
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.creating_folder = false;
                }
            });
            ui.separator();
        }

        // Rename inline editor
        if let Some(target) = self.rename_target.clone() {
            ui.horizontal(|ui| {
                ui.label(format!("Rename {}:", file_name_of(&target)));
                ui.text_edit_singleline(&mut self.rename_buffer);
                if ui.button("Rename").clicked() {
                    let dest = target.with_file_name(&self.rename_buffer);
                    match disk::move_path(&target, &dest, false) {
                        Ok(()) => {
                            self.status = format!("Renamed to {}", self.rename_buffer);
                            self.rename_target = None;
                            self.refresh();
                        }
                        Err(e) => self.status = format!("Rename failed: {e}"),
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.rename_target = None;
                }
            });
            ui.separator();
        }

        if self.current().loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading…");
            });
        }

        // Empty state
        if entries.is_empty() && !self.current().loading {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("This folder is empty").size(15.0).weak());
            });
            return;
        }

        match view {
            ViewMode::Details => self.details_view(ui, ctx, &entries),
            ViewMode::List => self.list_view(ui, &entries),
            ViewMode::Tiles => self.tiles_view(ui, ctx, &entries, 84.0),
            ViewMode::SmallIcons => self.tiles_view(ui, ctx, &entries, 56.0),
        }
    }

    fn details_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[DirEntry]) {
        let mut open: Option<DirEntry> = None;
        let mut menu: Option<(PathBuf, ActionRequest)> = None;
        let sort_key = self.current().sort_key;
        let sort_asc = self.current().sort_asc;

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(28.0).at_least(24.0).clip(true))
            .column(Column::initial(300.0).at_least(140.0).clip(true).resizable(true).range(120.0..=1200.0))
            .column(Column::initial(120.0).at_least(60.0).clip(true))
            .column(Column::initial(160.0).at_least(90.0).clip(true))
            .column(Column::remainder().at_least(80.0).clip(true))
            .min_scrolled_height(0.0);

        let clicked_cell = std::cell::Cell::new(None::<SortKeySerde>);
        let table = table.header(22.0, |mut header| {
            header.col(|ui| {
                ui.label("");
            });
            for (name, key) in [
                ("Name", SortKeySerde::Name),
                ("Size", SortKeySerde::Size),
                ("Date modified", SortKeySerde::Modified),
                ("Type", SortKeySerde::Type),
            ] {
                header.col(|ui| {
                    let arrow = if sort_key == key {
                        if sort_asc { " ▲" } else { " ▼" }
                    } else {
                        ""
                    };
                    if ui.add(egui::Button::new(RichText::new(format!("{name}{arrow}")).strong()).frame(false)).clicked() {
                        clicked_cell.set(Some(key));
                    }
                });
            }
        });

        let clicked_sort = clicked_cell.get();
        table.body(|body| {
            body.rows(24.0, entries.len(), |mut row| {
                let idx = row.index();
                let entry = &entries[idx];
                let selected = self.current().selected.as_deref() == Some(entry.path.as_path());
                row.set_selected(selected);

                row.col(|ui| {
                    let key = if entry.is_dir { "dir".to_string() } else { format!("ext:{}", disk::extension(&entry.name).to_lowercase()) };
                    let id = self.icon_for(ctx, &key, || {
                        let info = shell::file_info(&entry.path, true);
                        info.icon_rgba
                    });
                    if let Some(id) = id {
                        ui.add(egui::Image::new((id, egui::vec2(16.0, 16.0))));
                    } else {
                        ui.label(if entry.is_dir { "🗀" } else { "🗎" });
                    }
                });

                row.col(|ui| {
                    let name = &entry.name;
                    let text = RichText::new(name);
                    let resp = ui.selectable_label(selected, text);
                    if resp.clicked() {
                        self.current_mut().selected = Some(entry.path.clone());
                    }
                    if resp.double_clicked() {
                        open = Some(entry.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::Open));
                            ui.close_menu();
                        }
                        if !entry.is_dir {
                            if ui.button("Edit").clicked() {
                                menu = Some((entry.path.clone(), ActionRequest::Edit));
                                ui.close_menu();
                            }
                            if ui.button("Open with…").clicked() {
                                menu = Some((entry.path.clone(), ActionRequest::OpenWith));
                                ui.close_menu();
                            }
                        }
                        if !entry.is_dir && ui.button("Run as administrator").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::RunAsAdmin));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Rename").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::Rename));
                            ui.close_menu();
                        }
                        if ui.button("Delete").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::Delete));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Copy path").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::CopyPath));
                            ui.close_menu();
                        }
                        if ui.button("Properties").clicked() {
                            menu = Some((entry.path.clone(), ActionRequest::Properties));
                            ui.close_menu();
                        }
                    });
                });

                row.col(|ui| {
                    let size = if entry.is_dir { String::new() } else { disk::format_size(entry.size) };
                    ui.label(RichText::new(size).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(entry.modified_display()).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(disk::item_kind(entry)).weak());
                });
            });
        });

        if let Some(key) = clicked_sort {
            self.set_sort(key);
        }
        if let Some(entry) = open {
            self.open_entry(&entry);
        }
        if let Some((path, action)) = menu {
            self.handle_action(path, action);
        }
    }

    fn list_view(&mut self, ui: &mut egui::Ui, entries: &[DirEntry]) {
        let mut open: Option<DirEntry> = None;
        let mut menu: Option<(PathBuf, ActionRequest)> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width().max(600.0);
            let cols = ((width / 220.0).floor() as usize).max(1);
            for chunk in entries.chunks(cols) {
                ui.horizontal(|ui| {
                    for entry in chunk {
                        let selected = self.current().selected.as_deref() == Some(entry.path.as_path());
                        let label = format!("{} {}", if entry.is_dir { "🗀" } else { "🗎" }, entry.name);
                        let resp = ui.add_sized([width / cols as f32 - 8.0, 20.0], egui::Button::new(label).selected(selected).frame(false));
                        if resp.clicked() {
                            self.current_mut().selected = Some(entry.path.clone());
                        }
                        if resp.double_clicked() {
                            open = Some(entry.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                menu = Some((entry.path.clone(), ActionRequest::Open));
                                ui.close_menu();
                            }
                            if ui.button("Delete").clicked() {
                                menu = Some((entry.path.clone(), ActionRequest::Delete));
                                ui.close_menu();
                            }
                            if ui.button("Properties").clicked() {
                                menu = Some((entry.path.clone(), ActionRequest::Properties));
                                ui.close_menu();
                            }
                        });
                        // pad to keep columns aligned
                        let used = ui.available_width();
                        let _ = used;
                    }
                });
            }
        });
        if let Some(entry) = open {
            self.open_entry(&entry);
        }
        if let Some((path, action)) = menu {
            self.handle_action(path, action);
        }
    }

    fn tiles_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[DirEntry], tile_h: f32) {
        let mut open: Option<DirEntry> = None;
        let mut menu: Option<(PathBuf, ActionRequest)> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_width().max(320.0);
            let icon_size = tile_h - 34.0;
            let cols = ((avail / 128.0).floor() as usize).max(1);
            for chunk in entries.chunks(cols) {
                ui.horizontal(|ui| {
                    for entry in chunk {
                        let selected = self.current().selected.as_deref() == Some(entry.path.as_path());
                        ui.allocate_ui(egui::vec2(124.0, tile_h), |ui| {
                            ui.vertical_centered(|ui| {
                                let key = if entry.is_dir { "dir".to_string() } else { format!("ext:{}", disk::extension(&entry.name).to_lowercase()) };
                                let id = self.icon_for(ctx, &key, || {
                                    let info = shell::file_info(&entry.path, true);
                                    info.icon_rgba
                                });
                                let resp = if let Some(id) = id {
                                    ui.add(egui::ImageButton::new(egui::Image::new((id, egui::vec2(icon_size, icon_size)))).selected(selected))
                                } else {
                                    let t = if entry.is_dir { "🗀" } else { "🗎" };
                                    ui.add(egui::Button::new(RichText::new(t).size(icon_size * 0.7)).selected(selected))
                                };
                                if resp.clicked() {
                                    self.current_mut().selected = Some(entry.path.clone());
                                }
                                if resp.double_clicked() {
                                    open = Some(entry.clone());
                                }
                                resp.context_menu(|ui| {
                                    if ui.button("Open").clicked() {
                                        menu = Some((entry.path.clone(), ActionRequest::Open));
                                        ui.close_menu();
                                    }
                                    if ui.button("Delete").clicked() {
                                        menu = Some((entry.path.clone(), ActionRequest::Delete));
                                        ui.close_menu();
                                    }
                                    if ui.button("Properties").clicked() {
                                        menu = Some((entry.path.clone(), ActionRequest::Properties));
                                        ui.close_menu();
                                    }
                                });
                                let short = truncate_middle(&entry.name, 18);
                                ui.label(RichText::new(short).size(11.0));
                            });
                        });
                    }
                });
            }
        });
        if let Some(entry) = open {
            self.open_entry(&entry);
        }
        if let Some((path, action)) = menu {
            self.handle_action(path, action);
        }
    }

    fn handle_action(&mut self, path: PathBuf, action: ActionRequest) {
        match action {
            ActionRequest::Open => {
                if path.is_dir() {
                    self.navigate(path);
                } else if let Err(e) = shell::open(&path) {
                    self.status = format!("Open failed: {e}");
                }
            }
            ActionRequest::Edit => {
                if let Err(e) = shell::open_verb(&path, "edit") {
                    self.status = format!("Edit failed: {e}");
                }
            }
            ActionRequest::OpenWith => {
                if let Err(e) = shell::launch("rundll32.exe", &format!("shell32.dll,OpenAs_RunDLL {}", path.display())) {
                    self.status = format!("Open with failed: {e}");
                }
            }
            ActionRequest::RunAsAdmin => {
                if let Err(e) = shell::run_as_admin(&path) {
                    self.status = format!("Run as admin failed: {e}");
                }
            }
            ActionRequest::Rename => {
                self.rename_buffer = file_name_of(&path);
                self.rename_target = Some(path);
            }
            ActionRequest::Delete => {
                self.delete_confirm = vec![path];
            }
            ActionRequest::CopyPath => {
                self.status = format!("Path: {}", path.display());
            }
            ActionRequest::Properties => {
                if let Err(e) = shell::show_properties(&path) {
                    self.status = format!("Properties failed: {e}");
                }
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let tab = self.current();
            let files = tab.entries.iter().filter(|e| !e.is_dir).count();
            let dirs = tab.entries.iter().filter(|e| e.is_dir).count();
            let total: u64 = tab.entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
            ui.label(RichText::new(format!("{dirs} folders, {files} files · {}", disk::format_size(total))).small());
            if !self.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(&self.status).small().weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("WinRust {}", env!("CARGO_PKG_VERSION"))).small().weak());
            });
        });
    }

    fn process_events(&mut self, ctx: &egui::Context) {
        while let Ok(listing) = self.rx.try_recv() {
            if let Some(tab) = self.tabs.get_mut(listing.tab_index) {
                if tab.request_id == listing.request_id {
                    tab.loading = false;
                    match listing.result {
                        Ok(entries) => {
                            tab.entries = entries;
                            tab.error = None;
                        }
                        Err(e) => {
                            tab.entries.clear();
                            tab.error = Some(e);
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
        // Drive list refresh every 5s (USB sticks coming and going).
        if self.last_refresh.elapsed().as_secs() >= 5 {
            self.drives = disk::volumes();
            self.last_refresh = std::time::Instant::now();
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        let (f5, ctrl_t, ctrl_w, ctrl_l, del, f2, ctrl_c, alt_up) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F5),
                i.modifiers.ctrl && i.key_pressed(egui::Key::T),
                i.modifiers.ctrl && i.key_pressed(egui::Key::W),
                i.modifiers.ctrl && i.key_pressed(egui::Key::L),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::F2),
                i.modifiers.ctrl && i.key_pressed(egui::Key::C),
                i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if f5 {
            self.refresh();
        }
        if alt_up {
            self.go_up();
        }
        if ctrl_t {
            let p = self.current().path.clone();
            self.tabs.push(Tab::new(p));
            self.active = self.tabs.len() - 1;
            self.start_load(self.active);
        }
        if ctrl_w && self.tabs.len() > 1 {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len() - 1);
            self.start_load(self.active);
        }
        if ctrl_l {
            let p = self.current().path.display().to_string();
            self.current_mut().address_edit = Some(p);
        }
        if let Some(sel) = self.current().selected.clone() {
            if f2 {
                self.rename_buffer = file_name_of(&sel);
                self.rename_target = Some(sel.clone());
            }
            if del {
                self.delete_confirm = vec![sel.clone()];
            }
            if ctrl_c {
                self.status = format!("Selected: {}", sel.display());
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut remove: Option<usize> = None;
            let mut activate: Option<usize> = None;
            for (idx, tab) in self.tabs.iter().enumerate() {
                let selected = idx == self.active;
                let label = format!("{}  {}×", tab.title(), "");
                let resp = ui.selectable_label(selected, label.trim_end_matches('×').trim_end());
                if resp.clicked() {
                    activate = Some(idx);
                }
                if resp.middle_clicked() {
                    remove = Some(idx);
                }
                if tab.loading {
                    ui.spinner();
                }
            }
            if ui.button("+").on_hover_text("New tab (Ctrl+T)").clicked() {
                let p = self.current().path.clone();
                self.tabs.push(Tab::new(p));
                activate = Some(self.tabs.len() - 1);
            }
            if let Some(idx) = activate {
                self.active = idx;
                self.remember_prefs();
                self.start_load(self.active);
            }
            if let Some(idx) = remove {
                if self.tabs.len() > 1 {
                    self.tabs.remove(idx);
                    self.active = self.active.min(self.tabs.len() - 1);
                    self.start_load(self.active);
                }
            }
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        if !self.delete_confirm.is_empty() {
            let paths = self.delete_confirm.clone();
            egui::Window::new("Delete")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(if paths.len() == 1 {
                        format!("Move \"{}\" to the Recycle Bin?", file_name_of(&paths[0]))
                    } else {
                        format!("Move {} items to the Recycle Bin?", paths.len())
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            match shell::recycle(&paths) {
                                Ok(()) => {
                                    self.status = "Moved to Recycle Bin".into();
                                    self.delete_confirm.clear();
                                    self.refresh();
                                }
                                Err(e) => {
                                    self.status = format!("Delete failed: {e}");
                                    self.delete_confirm.clear();
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.delete_confirm.clear();
                        }
                    });
                });
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ActionRequest {
    Open,
    Edit,
    OpenWith,
    RunAsAdmin,
    Rename,
    Delete,
    CopyPath,
    Properties,
}

impl eframe::App for ExplorerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events(ctx);
        self.keyboard(ctx);

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_bar(ui));
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(230.0)
            .width_range(180.0..=380.0)
            .show(ctx, |ui| self.sidebar(ui, ctx));
        egui::CentralPanel::default().show(ctx, |ui| self.view_area(ui, ctx));

        self.modals(ctx);

        if self.current().loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember_prefs();
        let state = PersistedState {
            tabs: self.tabs.iter().map(|t| t.path.clone()).collect(),
            active_tab: self.active,
            folders: self.folders.clone(),
        };
        save_state(&state);
    }
}

/// Install a UI font that covers Cyrillic and CJK, falling back to the
/// bundled fonts when the system font cannot be read.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\SegoeUI.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("segoe".to_string(), egui::FontData::from_owned(bytes));
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "segoe".to_string());
            fonts.families.entry(egui::FontFamily::Monospace).or_default().push("segoe".to_string());
            break;
        }
    }
    ctx.set_fonts(fonts);
}

fn truncate_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

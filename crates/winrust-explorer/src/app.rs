//! Explorer window — a functional reimplementation of the Windows 10/11 file
//! browser.
//!
//! Design
//! ------
//! * `ExplorerApp` owns `Tab`s; every tab keeps its own history, sort order,
//!   view mode and multi-selection, exactly like Explorer windows.
//! * Directory listings run on a worker thread, so slow network shares never
//!   freeze the UI.  Only the newest request per tab is applied (stale results
//!   are dropped by request id), which also makes fast tab switching cheap.
//! * File operations (copy / move / delete) work on the whole selection and are
//!   cancellable through a shared flag.
//! * Icons, file types and folder aggregates are cached — the UI thread never
//!   calls the shell twice for the same key.
//! * Layout mirrors Explorer: command bar (New / Cut / Copy / Paste / Rename /
//!   Delete / View / Sort / options), breadcrumb address bar with search,
//!   navigation pane (Quick access, This PC drives, folders), details / list /
//!   tiles / content views, preview pane and status bar.

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use winrust_core::disk::{self, DirEntry, SortKey};
use winrust_core::i18n::{self, Lang};
use winrust_core::shell::{self, KnownFolder};
use winrust_core::{t, tf, ts};

// ---------------------------------------------------------------------------
// View modes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ViewMode {
    Details,
    List,
    Tiles,
    Content,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            ViewMode::Details => ts("Details"),
            ViewMode::List => ts("List"),
            ViewMode::Tiles => ts("Large icons"),
            ViewMode::Content => ts("Content"),
        }
    }
    const ALL: [ViewMode; 4] = [
        ViewMode::Details,
        ViewMode::List,
        ViewMode::Tiles,
        ViewMode::Content,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum SortKeySerde {
    Name,
    Size,
    Modified,
    Created,
    Type,
}

impl SortKeySerde {
    fn to_core(self) -> SortKey {
        match self {
            SortKeySerde::Name => SortKey::Name,
            SortKeySerde::Size => SortKey::Size,
            SortKeySerde::Modified => SortKey::Modified,
            SortKeySerde::Type => SortKey::Type,
            // Explorer sorts "Date created" and "Date modified" separately; the
            // core only exposes modified, so created falls back to it.
            SortKeySerde::Created => SortKey::Modified,
        }
    }
    fn label(self) -> &'static str {
        match self {
            SortKeySerde::Name => ts("Name"),
            SortKeySerde::Size => ts("Size"),
            SortKeySerde::Modified => ts("Date modified"),
            SortKeySerde::Created => ts("Date created"),
            SortKeySerde::Type => ts("Type"),
        }
    }
    const ALL: [SortKeySerde; 5] = [
        SortKeySerde::Name,
        SortKeySerde::Size,
        SortKeySerde::Modified,
        SortKeySerde::Created,
        SortKeySerde::Type,
    ];
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FolderPrefs {
    view: ViewMode,
    show_hidden: bool,
    sort_key: SortKeySerde,
    sort_ascending: bool,
    /// Icon-tile gap set through the background context menu.
    #[serde(default = "default_spacing")]
    spacing: f32,
    /// Tiles-view picture size, see [`Tab::icon_px`].
    #[serde(default = "default_icon_px")]
    icon_px: u32,
}

fn default_icon_px() -> u32 {
    48
}

fn default_spacing() -> f32 {
    12.0
}

impl Default for FolderPrefs {
    fn default() -> Self {
        Self {
            view: ViewMode::Details,
            show_hidden: false,
            sort_key: SortKeySerde::Name,
            sort_ascending: true,
            spacing: default_spacing(),
            icon_px: default_icon_px(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct PersistedState {
    tabs: Vec<PathBuf>,
    active_tab: usize,
    folders: HashMap<String, FolderPrefs>,
    #[serde(default)]
    preview_open: bool,
    #[serde(default)]
    sidebar_width: f32,
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

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

struct Tab {
    path: PathBuf,
    history: Vec<PathBuf>,
    history_pos: usize,
    entries: Vec<DirEntry>,
    loading: bool,
    error: Option<String>,
    /// Selected paths (Ctrl+click adds, Shift+click extends a range).
    selection: Vec<PathBuf>,
    /// Index in the sorted list that Shift+click extends from.
    anchor: Option<usize>,
    filter: String,
    address_edit: Option<String>,
    view: ViewMode,
    show_hidden: bool,
    sort_key: SortKeySerde,
    sort_asc: bool,
    request_id: u64,
    /// Scroll offset survives re-sorting so the view does not jump.
    scroll_to_top: bool,
    /// Gap between icon tiles in the icon views, in points.  Explorer never
    /// exposed this, the request explicitly asks for it.
    spacing: f32,
    /// Tiles view icon size: 48 (large), 32 (medium) or 24 (small).
    icon_px: u32,
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
            selection: Vec::new(),
            anchor: None,
            filter: String::new(),
            address_edit: None,
            view: ViewMode::Details,
            show_hidden: false,
            sort_key: SortKeySerde::Name,
            sort_asc: true,
            request_id: 0,
            scroll_to_top: false,
            spacing: 12.0,
            icon_px: 48,
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

    fn selected_one(&self) -> Option<&PathBuf> {
        if self.selection.len() == 1 {
            self.selection.first()
        } else {
            None
        }
    }

    fn is_selected(&self, path: &Path) -> bool {
        self.selection.iter().any(|p| p == path)
    }
}

/// Create an empty file with `extension` in `dir`, naming it the way Explorer's
/// "New -> ..." menu does (`New Text Document.txt`, then `(2)`, `(3)`, ...).
fn new_file_in(dir: &Path, extension: &str) -> Option<PathBuf> {
    let base = ts("New file");
    let ext = extension.trim_start_matches('.');
    let mut name = format!("{base}.{ext}");
    let mut n = 2;
    while dir.join(&name).exists() {
        name = format!("{base} ({n}).{ext}");
        n += 1;
    }
    let path = dir.join(&name);
    std::fs::File::create(&path).ok().map(|_| path)
}

fn file_name_of(p: &Path) -> String {
    match p.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() {
                "This PC".into()
            } else {
                s
            }
        }
    }
}

/// Message from the directory worker.
struct Listing {
    tab_index: usize,
    request_id: u64,
    result: Result<Vec<DirEntry>, String>,
}

/// A pending copy/move/delete job, run on a worker so the UI stays live.
struct FileJob {
    label: String,
    done: usize,
    total: usize,
    errors: Vec<String>,
    cancel: Arc<AtomicBool>,
    finished: bool,
    /// Set when the job needs the view to refresh afterwards.
    refresh_tab: usize,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct ExplorerApp {
    tabs: Vec<Tab>,
    active: usize,
    rx: Receiver<Listing>,
    tx: Sender<Listing>,
    next_request: u64,
    folders: HashMap<String, FolderPrefs>,
    status: String,
    /// Icon textures keyed by `"{size}:{kind}"` (dir / ext / path).
    icons: HashMap<String, Option<egui::TextureHandle>>,
    /// Explorer's localized "Type" text, resolved once per extension.
    type_names: HashMap<String, String>,
    /// Folder sizes for the status bar, filled by a background worker.
    folder_size: Option<(PathBuf, u64, u64)>,
    folder_size_pending: Option<PathBuf>,
    folder_size_rx: Receiver<(PathBuf, u64, u64)>,
    folder_size_tx: Sender<(PathBuf, u64, u64)>,
    nav_selected: Option<KnownFolder>,
    drives: Vec<disk::VolumeInfo>,
    rename_target: Option<PathBuf>,
    rename_buffer: String,
    creating_folder: bool,
    new_folder_buffer: String,
    delete_confirm: Vec<PathBuf>,
    delete_permanent: bool,
    /// Files staged by Ctrl+C / Ctrl+X, waiting for Ctrl+V.
    clipboard: Option<FileClipboard>,
    /// Drag & drop: the paths currently being dragged (set on drag start).
    drag_paths: Vec<PathBuf>,
    drag_active: bool,
    /// Folder hovered during a drag (drop target highlight).
    drop_target: Option<PathBuf>,
    job: Option<FileJob>,
    job_rx: Receiver<JobUpdate>,
    job_tx: Sender<JobUpdate>,
    preview_open: bool,
    sidebar_width: f32,
    /// Address-bar search results (whole-subtree name match).
    search_path: PathBuf,
    search_results: Option<Vec<DirEntry>>,
    search_rx: Receiver<(PathBuf, Vec<DirEntry>)>,
    search_tx: Sender<(PathBuf, Vec<DirEntry>)>,
    last_search: std::time::Instant,
    /// Ghost card that follows the cursor while dragging, by animation.
    drag_ghost: Option<DragGhost>,
    /// Global transparency/blur, shared with the other components.
    appearance: winrust_core::i18n::Appearance,
    appearance_dirty: bool,
    hwnd: isize,
    lang: Lang,
    /// When the registry was last polled for a language change.
    lang_checked_at: std::time::Instant,
}

/// The floating label shown under the cursor during a drag & drop.
#[derive(Debug, Clone)]
struct DragGhost {
    label: String,
    /// Short display name (mid-truncated, extension preserved) shown on the card.
    short: String,
    count: usize,
    /// Smoothed cursor position — the animation is this easing towards the
    /// real pointer, which is what makes the ghost feel attached to the hand.
    pos: egui::Pos2,
    spawned: std::time::Instant,
}

struct JobUpdate {
    done: usize,
    errors: Vec<String>,
    finished: bool,
}

/// A pending copy or move of Explorer-style file selections.
struct FileClipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        i18n::init();
        let appearance = i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);

        let persisted = load_state();
        let default_start = KnownFolder::Profile
            .path()
            .unwrap_or_else(|| PathBuf::from("C:\\"));
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
                        t.spacing = pref.spacing;
                        t.icon_px = pref.icon_px;
                    }
                    t
                })
                .collect()
        };
        let active = persisted.active_tab.min(tabs.len().saturating_sub(1));
        let (tx, rx) = channel();
        let (job_tx, job_rx) = channel();
        let (folder_size_tx, folder_size_rx) = channel();
        let (search_tx, search_rx) = channel();
        let mut app = Self {
            tabs,
            active,
            rx,
            tx,
            next_request: 1,
            folders: persisted.folders,
            status: String::new(),
            icons: HashMap::new(),
            type_names: HashMap::new(),
            folder_size: None,
            folder_size_pending: None,
            folder_size_rx,
            folder_size_tx,
            nav_selected: Some(KnownFolder::Profile),
            drives: disk::volumes(),
            rename_target: None,
            rename_buffer: String::new(),
            creating_folder: false,
            new_folder_buffer: String::new(),
            delete_confirm: Vec::new(),
            delete_permanent: false,
            clipboard: None,
            drag_paths: Vec::new(),
            drag_active: false,
            drop_target: None,
            job: None,
            job_rx,
            job_tx,
            preview_open: persisted.preview_open,
            sidebar_width: if persisted.sidebar_width > 0.0 {
                persisted.sidebar_width
            } else {
                230.0
            },
            search_path: PathBuf::new(),
            search_results: None,
            search_rx,
            search_tx,
            last_search: std::time::Instant::now(),
            drag_ghost: None,
            appearance,
            appearance_dirty: false,
            hwnd: 0,
            lang: i18n::current(),
            lang_checked_at: std::time::Instant::now(),
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

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// Kick off an async directory read for a tab.  Old results are discarded by
    /// comparing `request_id`, so rapid navigation never shows stale files.
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
        let read_path = path.clone();
        std::thread::spawn(move || {
            let result = disk::read_dir(&read_path, show_hidden).map_err(|e| e.to_string());
            let _ = tx.send(Listing {
                tab_index,
                request_id,
                result,
            });
        });

        // Folder sizes are only meaningful for single-folder views; ask the
        // background worker for the aggregate shown in the status bar.
        self.request_folder_size(path, show_hidden);
    }

    fn request_folder_size(&mut self, path: PathBuf, show_hidden: bool) {
        if self.folder_size_pending.as_ref() == Some(&path) {
            return;
        }
        self.folder_size_pending = Some(path.clone());
        let tx = self.folder_size_tx.clone();
        std::thread::spawn(move || {
            let mut budget = 20_000u32;
            let (bytes, count) = disk::folder_stats(&path, &mut budget, show_hidden);
            let _ = tx.send((path, bytes, count));
        });
    }

    fn navigate(&mut self, path: PathBuf) {
        if path.is_file() {
            self.select_in_parent(path);
            return;
        }
        let prefs = self
            .folders
            .get(&path.to_string_lossy().to_string())
            .cloned();
        {
            let tab = self.current_mut();
            tab.history.truncate(tab.history_pos + 1);
            tab.history.push(path.clone());
            tab.history_pos = tab.history.len() - 1;
            tab.path = path.clone();
            tab.selection.clear();
            tab.anchor = None;
            tab.filter.clear();
            tab.scroll_to_top = true;
            if let Some(p) = prefs {
                tab.view = p.view;
                tab.show_hidden = p.show_hidden;
                tab.sort_key = p.sort_key;
                tab.sort_asc = p.sort_ascending;
                tab.spacing = p.spacing;
                tab.icon_px = p.icon_px;
            }
        }
        self.search_results = None;
        self.remember_prefs();
        self.start_load(self.active);
    }

    /// Open the parent folder and select `path` (Explorer's "Show in folder").
    fn select_in_parent(&mut self, path: PathBuf) {
        if let Some(parent) = path.parent() {
            let parent = parent.to_path_buf();
            self.navigate(parent);
            self.current_mut().selection = vec![path.clone()];
            // Selection is applied once the listing arrives; remember the target.
            self.current_mut().anchor = None;
            self.status = format!("Selected {}", file_name_of(&path));
        }
    }

    fn go_back(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos > 0 {
            tab.history_pos -= 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selection.clear();
            tab.anchor = None;
        }
        self.start_load(self.active);
    }

    fn go_forward(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos + 1 < tab.history.len() {
            tab.history_pos += 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selection.clear();
            tab.anchor = None;
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
                    spacing: tab.spacing,
                    icon_px: tab.icon_px,
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

    // ------------------------------------------------------------------
    // Listing / filtering
    // ------------------------------------------------------------------

    fn sorted_filtered(&self) -> Vec<DirEntry> {
        let tab = self.current();
        // Search results take over the view when the user typed in the search box.
        if let Some(results) = &self.search_results {
            if !tab.filter.is_empty() {
                return results.clone();
            }
        }
        let needle = tab.filter.to_lowercase();
        let mut v: Vec<DirEntry> = if needle.is_empty() {
            tab.entries.clone()
        } else {
            tab.entries
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        };
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

    // ------------------------------------------------------------------
    // Icons
    // ------------------------------------------------------------------

    /// Load (once) and draw an icon for `entry`. `size` is 16, 32 or 48 px.
    fn icon_for(
        &mut self,
        ctx: &egui::Context,
        entry: &DirEntry,
        size: u32,
    ) -> Option<egui::TextureId> {
        let key = if entry.is_dir {
            format!("dir:{size}")
        } else if entry.ext.is_empty() {
            format!("file:{size}")
        } else {
            format!("ext:{}:{size}", entry.ext)
        };
        if let Some(slot) = self.icons.get(&key) {
            return slot.as_ref().map(|t| t.id());
        }
        let px = match size {
            0..=16 => 16,
            17..=32 => 32,
            _ => 48,
        };
        // One shell call returns both the pixels and the localized type name,
        // which is reused for every file with the same extension.
        let info = shell::file_info_sized(&entry.path, px);
        if !self.type_names.contains_key(&key) && !info.type_name.is_empty() {
            let type_key = if entry.is_dir {
                "dir".to_string()
            } else {
                entry.ext.clone()
            };
            self.type_names
                .entry(type_key)
                .or_insert(info.type_name.clone());
        }
        let texture = info.icon_rgba.and_then(|rgba| {
            let expected = (px * px * 4) as usize;
            if rgba.len() < expected {
                return None;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [px as usize, px as usize],
                &rgba[..expected],
            );
            Some(ctx.load_texture(key.clone(), image, egui::TextureOptions::LINEAR))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.icons.insert(key, texture);
        id
    }

    fn type_text(&mut self, entry: &DirEntry) -> String {
        let key = if entry.is_dir {
            "dir".to_string()
        } else {
            entry.ext.clone()
        };
        if let Some(t) = self.type_names.get(&key) {
            return t.clone();
        }
        let t = shell::type_name_for_extension(&entry.ext, entry.is_dir);
        self.type_names.insert(key, t.clone());
        t
    }
}

/// Install a UI font that covers Cyrillic and CJK, falling back to the bundled
/// fonts when the system font cannot be read.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\SegoeUI.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("segoe".to_string(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe".to_string());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("segoe".to_string());
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

// ---------------------------------------------------------------------------
// Commands (context menu / keyboard share these)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Open,
    OpenNewTab,
    Edit,
    OpenWith,
    RunAsAdmin,
    Copy,
    Cut,
    Paste,
    Rename,
    Delete,
    DeletePermanent,
    CopyPath,
    Properties,
    SendToDesktop,
    ExtractHere,
    SelectAll,
    InvertSelection,
    Refresh,
}

impl ExplorerApp {
    fn run(&mut self, path: PathBuf, cmd: Command) {
        match cmd {
            Command::Open => {
                if path.is_dir() {
                    self.navigate(path);
                } else if let Err(e) = shell::open(&path) {
                    self.status = format!("Open failed: {e}");
                }
            }
            Command::OpenNewTab => {
                if path.is_dir() {
                    self.tabs.push(Tab::new(path.clone()));
                    let idx = self.tabs.len() - 1;
                    self.active = idx;
                    self.start_load(idx);
                }
            }
            Command::Edit => {
                if let Err(e) = shell::open_verb(&path, "edit") {
                    self.status = format!("Edit failed: {e}");
                }
            }
            Command::OpenWith => {
                let args = format!("shell32.dll,OpenAs_RunDLL {}", path.display());
                if let Err(e) = shell::launch("rundll32.exe", &args) {
                    self.status = format!("Open with failed: {e}");
                }
            }
            Command::RunAsAdmin => {
                if let Err(e) = shell::run_as_admin(&path) {
                    self.status = format!("Run as admin failed: {e}");
                }
            }
            Command::Copy => {
                self.clipboard = Some(FileClipboard {
                    paths: vec![path.clone()],
                    cut: false,
                });
                self.status = format!("Copied: {}", file_name_of(&path));
            }
            Command::Cut => {
                self.clipboard = Some(FileClipboard {
                    paths: vec![path.clone()],
                    cut: true,
                });
                self.status = format!("Cut: {}", file_name_of(&path));
            }
            Command::Paste => self.paste_clipboard(),
            Command::Rename => {
                self.rename_buffer = file_name_of(&path);
                self.rename_target = Some(path);
            }
            Command::Delete => {
                self.delete_confirm = vec![path];
                self.delete_permanent = false;
            }
            Command::DeletePermanent => {
                self.delete_confirm = vec![path];
                self.delete_permanent = true;
            }
            Command::CopyPath => {
                let text = path.display().to_string();
                if shell::set_clipboard_text(&text) {
                    self.status = format!("Path copied: {text}");
                } else {
                    self.status = text;
                }
            }
            Command::Properties => {
                if let Err(e) = shell::show_properties(&path) {
                    self.status = format!("Properties failed: {e}");
                }
            }
            Command::SendToDesktop => {
                if let Some(desktop) = KnownFolder::Desktop.path() {
                    let name = file_name_of(&path);
                    let dest = disk::unique_destination(desktop.join(name));
                    match disk::copy_tree(&path, &dest, true) {
                        Ok(_) => self.status = format!("Shortcut copy → {}", dest.display()),
                        Err(e) => self.status = format!("Send to Desktop failed: {e}"),
                    }
                }
            }
            Command::ExtractHere => {
                // Explorer's "Extract All" for archives; Windows' built-in
                // zipfldr handler understands this verb.
                if let Err(e) = shell::open_verb(&path, "extract") {
                    self.status = format!("Extract failed: {e}");
                }
            }
            Command::SelectAll => {
                let entries = self.sorted_filtered();
                self.current_mut().selection = entries.into_iter().map(|e| e.path).collect();
                self.current_mut().anchor = None;
            }
            Command::InvertSelection => {
                let entries = self.sorted_filtered();
                let existing: HashSet<PathBuf> = self.current().selection.iter().cloned().collect();
                self.current_mut().selection = entries
                    .into_iter()
                    .map(|e| e.path)
                    .filter(|p| !existing.contains(p))
                    .collect();
            }
            Command::Refresh => self.refresh(),
        }
    }

    fn run_selection(&mut self, cmd: Command) {
        let sel: Vec<PathBuf> = self.current().selection.clone();
        if sel.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        match cmd {
            Command::Paste => self.paste_clipboard(),
            Command::SelectAll => self.run(PathBuf::new(), cmd),
            Command::InvertSelection => self.run(PathBuf::new(), cmd),
            Command::Refresh => self.refresh(),
            Command::Copy => {
                self.clipboard = Some(FileClipboard {
                    paths: sel.clone(),
                    cut: false,
                });
                self.status = format!("Copied {} item(s)", sel.len());
            }
            Command::Cut => {
                self.clipboard = Some(FileClipboard {
                    paths: sel.clone(),
                    cut: true,
                });
                self.status = format!("Cut {} item(s)", sel.len());
            }
            Command::Delete => {
                self.delete_confirm = sel;
                self.delete_permanent = false;
            }
            Command::DeletePermanent => {
                self.delete_confirm = sel;
                self.delete_permanent = true;
            }
            other => {
                for p in sel {
                    self.run(p, other);
                }
            }
        }
    }

    /// Ctrl+V — copy or move the staged entries into the current folder.
    /// Existing names get an Explorer-style `name (2).ext` suffix.
    fn paste_clipboard(&mut self) {
        let Some(clip) = self.clipboard.take() else {
            self.status = "Clipboard is empty".into();
            return;
        };
        let dest_dir = self.current().path.clone();
        if clip.paths.iter().any(|src| src == &dest_dir) {
            self.status = "Cannot paste a folder into itself".into();
            self.clipboard = Some(clip);
            return;
        }
        let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
        for src in &clip.paths {
            let Some(name) = src.file_name() else {
                continue;
            };
            let mut dest = dest_dir.join(name);
            if dest.exists() {
                dest = disk::unique_destination(&dest);
            }
            jobs.push((src.clone(), dest));
        }
        self.start_file_job(jobs, clip.cut, self.active);
    }

    /// Spawn a worker that performs `copy_tree` / `move_path` for every pair and
    /// streams progress back to the UI.
    fn start_file_job(&mut self, jobs: Vec<(PathBuf, PathBuf)>, cut: bool, tab: usize) {
        if jobs.is_empty() {
            return;
        }
        let verb = if cut { "Moving" } else { "Copying" };
        let label = format!("{verb} {} item(s)", jobs.len());
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(FileJob {
            label,
            done: 0,
            total: jobs.len(),
            errors: Vec::new(),
            cancel: cancel.clone(),
            finished: false,
            refresh_tab: tab,
        });
        let tx = self.job_tx.clone();
        std::thread::spawn(move || {
            let mut done = 0usize;
            let mut errors = Vec::new();
            for (src, dest) in jobs {
                if cancel.load(Ordering::Relaxed) {
                    errors.push("Cancelled".to_string());
                    break;
                }
                let result = if cut {
                    disk::move_path(&src, &dest, false)
                } else {
                    disk::copy_tree(&src, &dest, true).map(|_| ())
                };
                match result {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(format!("{}: {e}", file_name_of(&src))),
                }
                let _ = tx.send(JobUpdate {
                    done,
                    errors: errors.clone(),
                    finished: false,
                });
            }
            let _ = tx.send(JobUpdate {
                done,
                errors,
                finished: true,
            });
        });
    }

    fn pump_workers(&mut self, ctx: &egui::Context) {
        // Directory listings
        while let Ok(listing) = self.rx.try_recv() {
            if let Some(tab) = self.tabs.get_mut(listing.tab_index) {
                if tab.request_id == listing.request_id {
                    tab.loading = false;
                    match listing.result {
                        Ok(mut entries) => {
                            // Fill the localized type text once per extension.
                            for e in &mut entries {
                                let key = if e.is_dir {
                                    "dir".to_string()
                                } else {
                                    e.ext.clone()
                                };
                                if let Some(t) = self.type_names.get(&key) {
                                    e.type_name = t.clone();
                                } else {
                                    let t = shell::type_name_for_extension(&e.ext, e.is_dir);
                                    self.type_names.insert(key, t.clone());
                                    e.type_name = t;
                                }
                            }
                            tab.entries = entries;
                            tab.error = None;
                        }
                        Err(e) => {
                            tab.entries.clear();
                            tab.error = Some(e);
                        }
                    }
                    if tab.scroll_to_top {
                        tab.scroll_to_top = false;
                    }
                }
            }
        }

        // Folder aggregate sizes
        while let Ok((path, bytes, count)) = self.folder_size_rx.try_recv() {
            if self.folder_size_pending.as_ref() == Some(&path) {
                self.folder_size = Some((path.clone(), bytes, count));
                self.folder_size_pending = None;
            }
        }

        // File jobs
        while let Ok(update) = self.job_rx.try_recv() {
            if let Some(job) = &mut self.job {
                job.done = update.done;
                job.errors = update.errors;
                job.finished = update.finished;
                if update.finished {
                    let errors = job.errors.clone();
                    let done = job.done;
                    let refresh_tab = job.refresh_tab;
                    self.job = None;
                    self.status = if errors.is_empty() {
                        format!("Done: {done} item(s)")
                    } else {
                        format!(
                            "{done} done, {} error(s): {}",
                            errors.len(),
                            errors.join("; ")
                        )
                    };
                    if self.tabs.get(refresh_tab).is_some() {
                        self.start_load(refresh_tab);
                    }
                    // A successful cut consumes the clipboard.
                    if errors.is_empty() {
                        if let Some(c) = &mut self.clipboard {
                            if c.cut {
                                self.clipboard = None;
                            }
                        }
                    }
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint();
                }
            }
        }

        // Search results
        while let Ok((path, results)) = self.search_rx.try_recv() {
            if self.search_path == path {
                self.search_results = Some(results);
            }
        }
    }

    /// Ctrl+F / typing into the search box: match names in the current folder
    /// *and* its subfolders, like Explorer's search box.
    fn start_search(&mut self, needle: String) {
        self.current_mut().filter = needle.clone();
        if needle.is_empty() {
            self.search_results = None;
            return;
        }
        if self.last_search.elapsed().as_millis() < 150 {
            return;
        }
        self.last_search = std::time::Instant::now();
        let root = self.current().path.clone();
        self.search_path = root.clone();
        let tx = self.search_tx.clone();
        let show_hidden = self.current().show_hidden;
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut stack = vec![root.clone()];
            let mut budget = 40_000u32;
            let needle_l = needle.to_lowercase();
            while let Some(dir) = stack.pop() {
                if budget == 0 {
                    break;
                }
                let Ok(entries) = disk::read_dir(&dir, show_hidden) else {
                    continue;
                };
                for e in entries {
                    budget = budget.saturating_sub(1);
                    if budget == 0 {
                        break;
                    }
                    if e.name.to_lowercase().contains(&needle_l) {
                        out.push(e.clone());
                    }
                    if e.is_dir && !e.reparse {
                        stack.push(e.path.clone());
                    }
                }
                if out.len() > 2000 {
                    break;
                }
            }
            let _ = tx.send((root, out));
        });
    }
}

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

impl ExplorerApp {
    /// Explorer's tab strip (Win11 style) with a "+" button.
    fn tab_bar(&mut self, ui: &mut egui::Ui) {
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
    fn command_bar(&mut self, ui: &mut egui::Ui) {
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
    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut nav: Option<PathBuf> = None;
        let cur_path = self.current().path.clone();
        // Snapshot what the pane needs so `self` is not borrowed inside the
        // closure while it is also mutated by click handling.
        let subfolders: Vec<(String, PathBuf)> = self
            .current()
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .take(60)
            .map(|e| (e.name.clone(), e.path.clone()))
            .collect();
        let mut nav_selected = self.nav_selected;
        let mut status_msg: Option<String> = None;
        let drives = self.drives.clone();

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
                            None => status_msg = Some(tf!("{} is a virtual folder", label)),
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
                ui.label(RichText::new(t("FOLDERS IN THIS VIEW")).small().weak());
                if subfolders.is_empty() {
                    ui.label(RichText::new(t("   (no sub-folders)")).size(11.0).weak());
                }
                for (name, path) in &subfolders {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(format!("📁  {name}")).size(12.0))
                                .frame(false),
                        )
                        .clicked()
                    {
                        nav = Some(path.clone());
                    }
                }
            });
        let _ = ctx;
        self.nav_selected = nav_selected;
        if let Some(msg) = status_msg {
            self.status = msg;
        }
        if let Some(p) = nav {
            self.navigate(p);
        }
    }
}

fn sidebar_glyph(folder: KnownFolder) -> &'static str {
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

// ---------------------------------------------------------------------------
// File views
// ---------------------------------------------------------------------------

impl ExplorerApp {
    fn file_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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

    /// Is the active tab showing "This PC" (a drive root with no directory
    /// contents worth listing, i.e. the Computer known folder)?
    fn is_computer_view(&self) -> bool {
        let p = &self.current().path;
        if let Some(computer) = KnownFolder::Computer.path() {
            if p == &computer {
                return true;
            }
        }
        // Bare drive root: "C:\\"
        let s = p.to_string_lossy();
        s.len() == 3 && s.ends_with(":\\")
    }

    /// Shared selection logic: plain click replaces, Ctrl toggles, Shift extends.
    fn apply_click(
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
    }

    fn details_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[DirEntry]) {
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
                    if entry.is_dir && resp.hovered() && !self.drag_paths.is_empty() {
                        drop_on = Some(entry.path.clone());
                        ui.painter().rect_stroke(
                            resp.rect.expand(1.0),
                            2.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(0, 150, 255)),
                        );
                    }
                    if resp.drag_stopped() && entry.is_dir && !self.drag_paths.is_empty() {
                        drop_on = Some(entry.path.clone());
                    }
                    let mut local: Vec<Command> = Vec::new();
                    resp.context_menu(|ui| simple_context_menu(ui, entry, &mut local));
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

    fn list_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[DirEntry]) {
        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        let mut drag_started: Option<PathBuf> = None;
        let mut drop_on: Option<PathBuf> = None;
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
                                        simple_context_menu(ui, entry, &mut local)
                                    });
                                    commands
                                        .extend(local.into_iter().map(|c| (entry.path.clone(), c)));
                                    if resp.drag_started() {
                                        drag_started = Some(entry.path.clone());
                                    }
                                    if entry.is_dir && resp.hovered() && !self.drag_paths.is_empty()
                                    {
                                        drop_on = Some(entry.path.clone());
                                        ui.painter().rect_stroke(
                                            resp.rect.expand(1.0),
                                            2.0,
                                            egui::Stroke::new(
                                                1.5_f32,
                                                Color32::from_rgb(0, 150, 255),
                                            ),
                                        );
                                    }
                                    if resp.drag_stopped()
                                        && entry.is_dir
                                        && !self.drag_paths.is_empty()
                                    {
                                        drop_on = Some(entry.path.clone());
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
    fn tiles_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        entries: &[DirEntry],
        icon: u32,
    ) {
        let mut clicked: Option<(usize, PathBuf)> = None;
        let mut opened: Option<DirEntry> = None;
        let mut commands: Vec<(PathBuf, Command)> = Vec::new();
        let mut drag_started: Option<PathBuf> = None;
        let mut drop_on: Option<PathBuf> = None;
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
                                    let resp = if let Some(id) = id {
                                        ui.add(
                                            egui::ImageButton::new(egui::Image::new((
                                                id,
                                                egui::vec2(icon as f32, icon as f32),
                                            )))
                                            .selected(selected)
                                            .frame(false),
                                        )
                                    } else {
                                        let t = if entry.is_dir { "🗀" } else { "🗎" };
                                        ui.add(
                                            egui::Button::new(
                                                RichText::new(t).size(icon as f32 * 0.7),
                                            )
                                            .selected(selected)
                                            .frame(false),
                                        )
                                    };
                                    let mut local: Vec<Command> = Vec::new();
                                    resp.context_menu(|ui| {
                                        simple_context_menu(ui, entry, &mut local)
                                    });
                                    commands
                                        .extend(local.into_iter().map(|c| (entry.path.clone(), c)));
                                    if resp.drag_started() {
                                        drag_started = Some(entry.path.clone());
                                    }
                                    if entry.is_dir && resp.hovered() && !self.drag_paths.is_empty()
                                    {
                                        drop_on = Some(entry.path.clone());
                                        ui.painter().rect_stroke(
                                            resp.rect.expand(1.0),
                                            2.0,
                                            egui::Stroke::new(
                                                1.5_f32,
                                                Color32::from_rgb(0, 150, 255),
                                            ),
                                        );
                                    }
                                    if resp.drag_stopped()
                                        && entry.is_dir
                                        && !self.drag_paths.is_empty()
                                    {
                                        drop_on = Some(entry.path.clone());
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
    fn content_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[DirEntry]) {
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
                    resp.context_menu(|ui| simple_context_menu(ui, entry, &mut local));
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
fn simple_context_menu(ui: &mut egui::Ui, entry: &DirEntry, out: &mut Vec<Command>) {
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
        if ui.button(t("Open with…")).clicked() {
            out.push(Command::OpenWith);
            ui.close_menu();
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
fn format_date(ft: u64) -> String {
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
    fn finish_drag_and_commands(
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
        if self.current().selection.is_empty() && self.drag_active {
            self.drag_active = false;
            self.drag_paths.clear();
        }
        if self.job.is_some() || self.current().loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}

// ---------------------------------------------------------------------------
// Preview pane / status bar
// ---------------------------------------------------------------------------

impl ExplorerApp {
    fn preview_pane(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
    fn icon_for_path(
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

    fn status_bar(&mut self, ui: &mut egui::Ui) {
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

fn plural(n: u64) -> String {
    format!("{n} item(s)")
}

fn view_glyph(v: ViewMode) -> &'static str {
    match v {
        ViewMode::Details => "☰",
        ViewMode::List => "⋯",
        ViewMode::Tiles => "▦",
        ViewMode::Content => "▤",
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
                    let mut errors = Vec::new();
                    for p in &paths {
                        let result = if permanent {
                            disk::delete_permanent(p)
                        } else {
                            shell::recycle(std::slice::from_ref(p))
                        };
                        if let Err(e) = result {
                            errors.push(format!("{}: {e}", file_name_of(p)));
                        }
                    }
                    self.status = if errors.is_empty() {
                        if permanent {
                            format!("Deleted {} item(s)", paths.len())
                        } else {
                            format!("Moved {} item(s) to the Recycle Bin", paths.len())
                        }
                    } else {
                        format!("{} error(s): {}", errors.len(), errors.join("; "))
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

impl ExplorerApp {
    /// Animate and draw the drag & drop ghost.
    ///
    /// The ghost eases towards the pointer (exponential smoothing at ~30 fps),
    /// lifts with a small scale-in over 120 ms, and fades out on drop.  That is
    /// the whole "animated drag and drop" requirement: the card visibly follows
    /// the hand instead of teleporting, and the drop target pulses in response.
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

    /// This PC's "Devices and drives" section: a tile per volume with the same
    /// text Explorer prints under each one ("117 ГБ свободно из 228 ГБ").
    fn drives_section(&mut self, ui: &mut egui::Ui) {
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
                    if resp.clicked() {
                        nav = Some(PathBuf::from(&drive.root));
                    }
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
    fn folders_section(&mut self, ui: &mut egui::Ui) {
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
                    // Folders are drop targets here too, with the same
                    // highlight animation as in the list views.
                    if self.drag_active && resp.hovered() {
                        painter.rect_stroke(
                            rect.expand(1.0),
                            4.0,
                            egui::Stroke::new(1.5_f32, Color32::from_rgb(96, 176, 255)),
                        );
                        if resp.drag_stopped() {
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
                    ui.set_min_width(220.0);
                    if ui.button(t("Folder")).clicked() {
                        self.creating_folder = true;
                        self.new_folder_buffer = t("New folder");
                        ui.close_menu();
                    }
                    if ui.button(t("Text document")).clicked() {
                        new_file_in(&self.current().path, "txt");
                        cmds.push(Command::Refresh);
                        ui.close_menu();
                    }
                    if ui.button(t("Bitmap image")).clicked() {
                        new_file_in(&self.current().path, "bmp");
                        cmds.push(Command::Refresh);
                        ui.close_menu();
                    }
                    if ui.button(t("ZIP archive")).clicked() {
                        new_file_in(&self.current().path, "zip");
                        cmds.push(Command::Refresh);
                        ui.close_menu();
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
                if matches!(c, Command::SendToDesktop) {
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
        };
        save_state(&state);
    }
}

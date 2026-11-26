//! The window itself: [`ExplorerApp`], its construction, navigation verbs,
//! and the caches that make a directory listing cheap to draw twice.

use super::tabs::{file_name_of, FileJob, Listing, Special, Tab};
use super::viewmode::{load_state, FolderPrefs, SortKeySerde};
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use winrust_core::disk::{self, DirEntry};
use winrust_core::i18n::{self, Lang};
use winrust_core::shell;
use winrust_core::shell::KnownFolder;

pub struct ExplorerApp {
    pub(super) tabs: Vec<Tab>,
    pub(super) active: usize,
    /// Every background channel the window owns, behind verbs.  These used to
    /// be eight public fields (`rx`, `tx`, `job_rx`, `job_tx`, `search_rx`,
    /// `search_tx`, `folder_size_rx`, `folder_size_tx`) that only
    /// `pump_workers` and the two `spawn` sites ever touched - a textbook
    /// field leak: the wiring was visible to the whole module and nothing
    /// stopped a third site from reading a half-applied queue.
    pub(super) workers: Workers,
    pub(super) next_request: u64,
    pub(super) folders: HashMap<String, FolderPrefs>,
    pub(super) status: String,
    /// Icon textures keyed by `"{size}:{kind}"` (dir / ext / path).
    pub(super) icons: HashMap<String, Option<egui::TextureHandle>>,
    /// Explorer's localized "Type" text, resolved once per extension.
    pub(super) type_names: HashMap<String, String>,
    /// Folder sizes for the status bar, filled by a background worker.
    pub(super) folder_size: Option<(PathBuf, u64, u64)>,
    pub(super) folder_size_pending: Option<PathBuf>,
    pub(super) nav_selected: Option<KnownFolder>,
    /// Folders pinned to the left pane's favourites list.  Explorer has no
    /// "quick access" of its own here, so this *is* that list: the user drags a
    /// folder onto the pane and it stays.
    pub(super) pinned: Vec<PathBuf>,
    pub(super) drives: Vec<disk::VolumeInfo>,
    pub(super) rename_target: Option<PathBuf>,
    pub(super) rename_buffer: String,
    pub(super) creating_folder: bool,
    pub(super) new_folder_buffer: String,
    pub(super) delete_confirm: Vec<PathBuf>,
    pub(super) delete_permanent: bool,
    /// Files staged by Ctrl+C / Ctrl+X, waiting for Ctrl+V.
    pub(super) clipboard: Option<FileClipboard>,
    /// Drag & drop: the paths currently being dragged (set on drag start).
    pub(super) drag_paths: Vec<PathBuf>,
    pub(super) drag_active: bool,
    /// Folder hovered during a drag (drop target highlight).
    pub(super) drop_target: Option<PathBuf>,
    pub(super) job: Option<FileJob>,
    pub(super) preview_open: bool,
    pub(super) sidebar_width: f32,
    /// Address-bar search results (whole-subtree name match).
    pub(super) search_path: PathBuf,
    pub(super) search_results: Option<Vec<DirEntry>>,
    pub(super) last_search: std::time::Instant,
    /// Ghost card that follows the cursor while dragging, by animation.
    pub(super) drag_ghost: Option<DragGhost>,
    /// Drag image style, see [`PersistedState::minimal_drag_image`].
    pub(super) minimal_drag_image: bool,
    /// Where a rubber-band selection started, in screen coordinates.  Set on a
    /// press over empty space in the file area, cleared on release.
    pub(super) band_start: Option<egui::Pos2>,
    /// Screen rectangles of the rows drawn this frame, so the rubber band can
    /// ask "which entries did the box touch?" using what is actually on screen
    /// instead of guessing row heights.  Cleared by each view as it starts.
    pub(super) row_rects: Vec<(PathBuf, egui::Rect)>,
    /// Global transparency/blur, shared with the other components.
    pub(super) appearance: winrust_core::i18n::Appearance,
    pub(super) appearance_dirty: bool,
    pub(super) hwnd: isize,
    pub(super) lang: Lang,
    /// When the registry was last polled for a language change.
    pub(super) lang_checked_at: std::time::Instant,
    /// "New" submenu contents, read from the registry when the menu is opened
    /// (never every frame - each entry costs a registry walk).
    pub(super) new_items: Vec<winrust_core::fileassoc::NewItem>,
    pub(super) new_items_at: Option<std::time::Instant>,
    /// "Open with" entries for the current selection, same policy.
    pub(super) open_with: Vec<winrust_core::fileassoc::OpenWith>,
}

/// The floating label shown under the cursor during a drag & drop.
#[derive(Debug, Clone)]
/// The window's background workers, behind one narrow interface.
///
/// A directory read, a folder-size tally and a recursive search each run on
/// their own thread and report back over a channel; a file job streams its
/// progress the same way.  Nothing outside this type needs to know that, so
/// nothing outside this type can see a `Receiver`: the four queues are
/// private, and the shell drains them with the `take_*` verbs.
pub(super) struct Workers {
    listing_tx: Sender<Listing>,
    listing_rx: Receiver<Listing>,
    job_tx: Sender<JobUpdate>,
    job_rx: Receiver<JobUpdate>,
    folder_size_tx: Sender<(PathBuf, u64, u64)>,
    folder_size_rx: Receiver<(PathBuf, u64, u64)>,
    search_tx: Sender<(PathBuf, Vec<DirEntry>)>,
    search_rx: Receiver<(PathBuf, Vec<DirEntry>)>,
}

impl Workers {
    fn new() -> Self {
        let (listing_tx, listing_rx) = channel();
        let (job_tx, job_rx) = channel();
        let (folder_size_tx, folder_size_rx) = channel();
        let (search_tx, search_rx) = channel();
        Self {
            listing_tx,
            listing_rx,
            job_tx,
            job_rx,
            folder_size_tx,
            folder_size_rx,
            search_tx,
            search_rx,
        }
    }

    // --- Senders handed to a worker thread -------------------------------

    pub(super) fn listing_sender(&self) -> Sender<Listing> {
        self.listing_tx.clone()
    }

    pub(super) fn folder_size_sender(&self) -> Sender<(PathBuf, u64, u64)> {
        self.folder_size_tx.clone()
    }

    pub(super) fn job_sender(&self) -> Sender<JobUpdate> {
        self.job_tx.clone()
    }

    pub(super) fn search_sender(&self) -> Sender<(PathBuf, Vec<DirEntry>)> {
        self.search_tx.clone()
    }

    // --- Receivers, drained by the shell ---------------------------------

    /// Listings that finished since the last frame.
    pub(super) fn take_listings(&self) -> Vec<Listing> {
        self.listing_rx.try_iter().collect()
    }

    /// Folder-size tallies that finished since the last frame.
    pub(super) fn take_folder_sizes(&self) -> Vec<(PathBuf, u64, u64)> {
        self.folder_size_rx.try_iter().collect()
    }

    /// File-job progress that arrived since the last frame.
    pub(super) fn take_job_updates(&self) -> Vec<JobUpdate> {
        self.job_rx.try_iter().collect()
    }

    /// Search results that arrived since the last frame.
    pub(super) fn take_search_results(&self) -> Vec<(PathBuf, Vec<DirEntry>)> {
        self.search_rx.try_iter().collect()
    }
}

pub(super) struct DragGhost {
    pub(super) label: String,
    /// Short display name (mid-truncated, extension preserved) shown on the card.
    pub(super) short: String,
    pub(super) count: usize,
    /// Smoothed cursor position — the animation is this easing towards the
    /// real pointer, which is what makes the ghost feel attached to the hand.
    pub(super) pos: egui::Pos2,
    pub(super) spawned: std::time::Instant,
}

pub(super) struct JobUpdate {
    pub(super) done: usize,
    pub(super) errors: Vec<String>,
    pub(super) finished: bool,
}

/// A pending copy or move of Explorer-style file selections.
pub(super) struct FileClipboard {
    pub(super) paths: Vec<PathBuf>,
    pub(super) cut: bool,
}

impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        i18n::init();
        let appearance = i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);

        let persisted = load_state();
        // Explorer opens on "This PC", as Windows does, so the first thing the
        // user sees is the drives.  It used to open the profile folder.
        let tabs: Vec<Tab> = if persisted.tabs.is_empty() {
            vec![Tab::special(Special::Computer)]
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
        let pinned = persisted.pinned.clone();
        // Four channel pairs, one owner: `Workers`.
        let workers = Workers::new();
        let mut app = Self {
            tabs,
            active,
            workers,
            next_request: 1,
            folders: persisted.folders,
            status: String::new(),
            icons: HashMap::new(),
            type_names: HashMap::new(),
            folder_size: None,
            folder_size_pending: None,
            nav_selected: Some(KnownFolder::Computer),
            pinned,
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
            preview_open: persisted.preview_open,
            sidebar_width: if persisted.sidebar_width > 0.0 {
                persisted.sidebar_width
            } else {
                230.0
            },
            search_path: PathBuf::new(),
            search_results: None,
            last_search: std::time::Instant::now(),
            drag_ghost: None,
            minimal_drag_image: persisted.minimal_drag_image,
            band_start: None,
            row_rects: Vec::new(),
            appearance,
            appearance_dirty: false,
            hwnd: 0,
            lang: i18n::current(),
            lang_checked_at: std::time::Instant::now(),
            new_items: Vec::new(),
            new_items_at: None,
            open_with: Vec::new(),
        };
        app.start_load(app.active);
        app
    }

    pub(super) fn current(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub(super) fn current_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// Kick off an async directory read for a tab.  Old results are discarded by
    /// comparing `request_id`, so rapid navigation never shows stale files.
    pub(super) fn start_load(&mut self, tab_index: usize) {
        let (path, show_hidden, special) = {
            let tab = &self.tabs[tab_index];
            (tab.path.clone(), tab.show_hidden, tab.special)
        };
        // A virtual folder has no directory to read: the view draws its own
        // contents (drive tiles, network shares, recycled items) and the tab
        // must not sit in a permanent "Loading".
        if special.is_some() {
            self.tabs[tab_index].loading = false;
            self.tabs[tab_index].error = None;
            self.tabs[tab_index].entries.clear();
            return;
        }
        self.next_request += 1;
        let request_id = self.next_request;
        self.tabs[tab_index].request_id = request_id;
        self.tabs[tab_index].loading = true;
        self.tabs[tab_index].error = None;

        let tx = self.workers.listing_sender();
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

    pub(super) fn request_folder_size(&mut self, path: PathBuf, show_hidden: bool) {
        if self.folder_size_pending.as_ref() == Some(&path) {
            return;
        }
        self.folder_size_pending = Some(path.clone());
        let tx = self.workers.folder_size_sender();
        std::thread::spawn(move || {
            let mut budget = 20_000u32;
            let (bytes, count) = disk::folder_stats(&path, &mut budget, show_hidden);
            let _ = tx.send((path, bytes, count));
        });
    }

    /// Navigate the active tab to a virtual shell folder.
    ///
    /// Separate from [`Self::navigate`] because there is no path to read:
    /// the tab keeps the `shell:` string for its address and title, and the
    /// view draws the folder's own layout.
    pub(super) fn navigate_special(&mut self, special: super::tabs::Special) {
        let path = PathBuf::from(special.shell_path());
        {
            let tab = self.current_mut();
            tab.history.truncate(tab.history_pos + 1);
            tab.history.push(path.clone());
            tab.history_pos = tab.history.len() - 1;
            tab.path = path;
            tab.special = Some(special);
            tab.entries.clear();
            tab.loading = false;
            tab.error = None;
            tab.selection.clear();
            tab.anchor = None;
            tab.filter.clear();
            tab.scroll_to_top = true;
        }
        self.search_results = None;
        self.remember_prefs();
    }

    pub(super) fn navigate(&mut self, path: PathBuf) {
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
            // Navigating to a real path leaves the virtual folder behind.
            tab.special = None;
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
    pub(super) fn select_in_parent(&mut self, path: PathBuf) {
        if let Some(parent) = path.parent() {
            let parent = parent.to_path_buf();
            self.navigate(parent);
            self.current_mut().selection = vec![path.clone()];
            // Selection is applied once the listing arrives; remember the target.
            self.current_mut().anchor = None;
            self.status = format!("Selected {}", file_name_of(&path));
        }
    }

    pub(super) fn go_back(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos > 0 {
            tab.history_pos -= 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selection.clear();
            tab.anchor = None;
        }
        self.start_load(self.active);
    }

    pub(super) fn go_forward(&mut self) {
        let tab = self.current_mut();
        if tab.history_pos + 1 < tab.history.len() {
            tab.history_pos += 1;
            tab.path = tab.history[tab.history_pos].clone();
            tab.selection.clear();
            tab.anchor = None;
        }
        self.start_load(self.active);
    }

    pub(super) fn go_up(&mut self) {
        let parent = self.current().path.parent().map(|p| p.to_path_buf());
        if let Some(p) = parent {
            self.navigate(p);
        }
    }

    pub(super) fn remember_prefs(&mut self) {
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

    pub(super) fn refresh(&mut self) {
        self.start_load(self.active);
    }

    pub(super) fn set_sort(&mut self, key: SortKeySerde) {
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

    pub(super) fn sorted_filtered(&self) -> Vec<DirEntry> {
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

    pub(super) fn open_entry(&mut self, entry: &DirEntry) {
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
    pub(super) fn icon_for(
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

    pub(super) fn type_text(&mut self, entry: &DirEntry) -> String {
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
pub(super) fn install_fonts(ctx: &egui::Context) {
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

pub(super) fn truncate_middle(s: &str, max_chars: usize) -> String {
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

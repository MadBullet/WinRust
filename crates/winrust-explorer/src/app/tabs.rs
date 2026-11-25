//! One browser tab: its path, history, listing and selection, plus the
//! messages the worker thread sends back.

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use winrust_core::disk::{self, DirEntry, SortKey};
use winrust_core::ts;

pub(super) struct Tab {
    pub(super) path: PathBuf,
    pub(super) history: Vec<PathBuf>,
    pub(super) history_pos: usize,
    pub(super) entries: Vec<DirEntry>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    /// Selected paths (Ctrl+click adds, Shift+click extends a range).
    pub(super) selection: Vec<PathBuf>,
    /// Index in the sorted list that Shift+click extends from.
    pub(super) anchor: Option<usize>,
    pub(super) filter: String,
    pub(super) address_edit: Option<String>,
    pub(super) view: ViewMode,
    pub(super) show_hidden: bool,
    pub(super) sort_key: SortKeySerde,
    pub(super) sort_asc: bool,
    pub(super) request_id: u64,
    /// Scroll offset survives re-sorting so the view does not jump.
    pub(super) scroll_to_top: bool,
    /// Gap between icon tiles in the icon views, in points.  Explorer never
    /// exposed this, the request explicitly asks for it.
    pub(super) spacing: f32,
    /// Tiles view icon size: 48 (large), 32 (medium) or 24 (small).
    pub(super) icon_px: u32,
}

impl Tab {
    pub(super) fn new(path: PathBuf) -> Self {
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

    pub(super) fn title(&self) -> String {
        file_name_of(&self.path)
    }

    pub(super) fn can_back(&self) -> bool {
        self.history_pos > 0
    }

    pub(super) fn can_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    pub(super) fn selected_one(&self) -> Option<&PathBuf> {
        if self.selection.len() == 1 {
            self.selection.first()
        } else {
            None
        }
    }

    pub(super) fn is_selected(&self, path: &Path) -> bool {
        self.selection.iter().any(|p| p == path)
    }
}

/// Create an empty file with `extension` in `dir`, naming it the way Explorer's
/// "New -> ..." menu does (`New Text Document.txt`, then `(2)`, `(3)`, ...).
pub(super) fn new_file_in(dir: &Path, extension: &str) -> Option<PathBuf> {
    // Naming, numbering and the `ShellNew` template all live in
    // `winrust_core::files`, so the desktop and this window cannot disagree.
    winrust_core::files::create_file(dir, extension, ts("New file")).ok()
}

pub(super) fn file_name_of(p: &Path) -> String {
    winrust_core::disk::file_name_of(p)
}

/// Message from the directory worker.
pub(super) struct Listing {
    pub(super) tab_index: usize,
    pub(super) request_id: u64,
    pub(super) result: Result<Vec<DirEntry>, String>,
}

/// A pending copy/move/delete job, run on a worker so the UI stays live.
pub(super) struct FileJob {
    pub(super) label: String,
    pub(super) done: usize,
    pub(super) total: usize,
    pub(super) errors: Vec<String>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) finished: bool,
    /// Set when the job needs the view to refresh afterwards.
    pub(super) refresh_tab: usize,
}

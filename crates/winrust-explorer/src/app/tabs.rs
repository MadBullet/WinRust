//! One browser tab: its path, history, listing and selection, plus the
//! messages the worker thread sends back.

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use winrust_core::disk::DirEntry;
use winrust_core::shell::KnownFolder;
use winrust_core::ts;

use super::viewmode::{SortKeySerde, ViewMode};

pub(super) struct Tab {
    pub(super) path: PathBuf,
    /// Set when the tab shows a *virtual* shell folder (This PC, Network, the
    /// Recycle Bin) rather than a directory.  Such a folder has no filesystem
    /// path: `path` then carries the `shell:` namespace string so history,
    /// the address bar and the tab strip have something stable to show, and
    /// `start_load` skips the directory read.
    pub(super) special: Option<Special>,
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

/// The virtual shell folders the Explorer can show.
///
/// They exist only inside the shell namespace: `SHGetKnownFolderPath` returns
/// a real directory for `FOLDERID_ComputerFolder` (the profile on most
/// machines), which is why comparing paths alone cannot tell "This PC" from
/// the profile folder - the earlier code did exactly that and turned a drive
/// double-click into the same drive grid.  A tab now says which of them it
/// shows, and the Explorer Home layout keys off that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Special {
    Computer,
    Network,
    RecycleBin,
}

impl Special {
    /// The namespace string kept in [`Tab::path`] while the tab shows this
    /// folder.  It is what the address bar prints and what a reopened tab
    /// matches on.
    pub(super) fn shell_path(self) -> &'static str {
        match self {
            Special::Computer => "shell:MyComputerFolder",
            Special::Network => "shell:NetworkPlacesFolder",
            Special::RecycleBin => "shell:RecycleBinFolder",
        }
    }

    /// The tab title.
    pub(super) fn title(self) -> &'static str {
        match self {
            Special::Computer => "This PC",
            Special::Network => "Network",
            Special::RecycleBin => "Recycle Bin",
        }
    }

    /// The tab for `folder`, if it is a virtual one.
    pub(super) fn of(folder: KnownFolder) -> Option<Self> {
        match folder {
            KnownFolder::Computer => Some(Special::Computer),
            KnownFolder::Network => Some(Special::Network),
            KnownFolder::RecycleBin => Some(Special::RecycleBin),
            _ => None,
        }
    }
}

impl Tab {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path: path.clone(),
            special: None,
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

    /// A tab for a virtual folder ("This PC", "Network", "Recycle Bin").
    ///
    /// Its `path` is the `shell:` namespace string rather than a real
    /// directory: `shell:` namespaces have no filesystem path, and the folder
    /// list is produced by the shell instead of `read_dir`.
    pub(super) fn special(special: Special) -> Self {
        let mut tab = Self::new(PathBuf::from(special.shell_path()));
        tab.special = Some(special);
        tab
    }

    pub(super) fn title(&self) -> String {
        // A virtual folder's `path` is a `shell:` namespace string, which
        // `file_name_of` would print verbatim; its own name is the title.
        if let Some(special) = self.special {
            return ts(special.title()).to_string();
        }
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

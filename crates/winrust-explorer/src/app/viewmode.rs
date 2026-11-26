//! View modes, sort orders and the per-folder preferences that the
//! explorer window remembers between runs (`%APPDATA%\WinRust`).

use std::collections::HashMap;
use std::path::PathBuf;
use winrust_core::disk::SortKey;
use winrust_core::ts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum ViewMode {
    Details,
    List,
    Tiles,
    Content,
}

impl ViewMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            ViewMode::Details => ts("Details"),
            ViewMode::List => ts("List"),
            ViewMode::Tiles => ts("Large icons"),
            ViewMode::Content => ts("Content"),
        }
    }
    pub(super) const ALL: [ViewMode; 4] = [
        ViewMode::Details,
        ViewMode::List,
        ViewMode::Tiles,
        ViewMode::Content,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum SortKeySerde {
    Name,
    Size,
    Modified,
    Created,
    Type,
}

impl SortKeySerde {
    pub(super) fn to_core(self) -> SortKey {
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
    pub(super) fn label(self) -> &'static str {
        match self {
            SortKeySerde::Name => ts("Name"),
            SortKeySerde::Size => ts("Size"),
            SortKeySerde::Modified => ts("Date modified"),
            SortKeySerde::Created => ts("Date created"),
            SortKeySerde::Type => ts("Type"),
        }
    }
    pub(super) const ALL: [SortKeySerde; 5] = [
        SortKeySerde::Name,
        SortKeySerde::Size,
        SortKeySerde::Modified,
        SortKeySerde::Created,
        SortKeySerde::Type,
    ];
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct FolderPrefs {
    pub(super) view: ViewMode,
    pub(super) show_hidden: bool,
    pub(super) sort_key: SortKeySerde,
    pub(super) sort_ascending: bool,
    /// Icon-tile gap set through the background context menu.
    #[serde(default = "default_spacing")]
    pub(super) spacing: f32,
    /// Tiles-view picture size, see [`Tab::icon_px`].
    #[serde(default = "default_icon_px")]
    pub(super) icon_px: u32,
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
pub(super) struct PersistedState {
    pub(super) tabs: Vec<PathBuf>,
    pub(super) active_tab: usize,
    pub(super) folders: HashMap<String, FolderPrefs>,
    #[serde(default)]
    pub(super) preview_open: bool,
    #[serde(default)]
    pub(super) sidebar_width: f32,
    /// Folders the user pinned into the left pane's favourites list.
    #[serde(default)]
    pub(super) pinned: Vec<PathBuf>,
}

pub(super) fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(base).join("WinRust");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("explorer.json"))
}

pub(super) fn load_state() -> PersistedState {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(super) fn save_state(state: &PersistedState) {
    if let Some(p) = state_path() {
        if let Ok(s) = serde_json::to_string_pretty(state) {
            let _ = std::fs::write(p, s);
        }
    }
}

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

//! Modules
//! -------
//! * [`viewmode`] - view modes, sort orders, persisted folder preferences
//! * [`tabs`]     - one tab's path, history, listing and selection
//! * [`state`]    - `ExplorerApp` itself: construction, navigation, caches
//! * [`commands`] - the file verbs the menus and the keyboard share
//! * [`panels`]   - tab strip, command bar, navigation pane
//! * [`sidebar`]  - the drives and pinned folders in the pane
//! * [`views`]    - details / list / tiles / content, and drag & drop
//! * [`preview`]  - preview pane and status bar
//! * [`keyboard`] - keys, modals, and the window loop

mod commands;
mod keyboard;
mod panels;
mod preview;
mod sidebar;
mod state;
mod tabs;
mod viewmode;
mod views;

pub use state::ExplorerApp;

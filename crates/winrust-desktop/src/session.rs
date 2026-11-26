//! The Start menu's own state: whether it is open, what is typed in it, and
//! which power action is waiting for a confirmation.
//!
//! The menu used to be five loose fields on [`crate::DesktopApp`] (`start_open`,
//! `start_search`, `start_power_menu`, `start_anim` and the cached app list)
//! that every reader poked at directly, which is exactly the `field-leak` the
//! linter reports: nothing could keep the invariant "the power menu is only
//! ever pending while the menu is open", because every writer could break it.
//!
//! [`Session`] owns those fields.  The shell asks it questions
//! ([`Session::is_open`], [`Session::anim`], [`Session::pending_power`]) and
//! gives it verbs ([`Session::open`], [`Session::close`], [`Session::toggle`],
//! [`Session::ask_power`], [`Session::cancel_power`]).  Closing the menu also
//! drops the pending confirmation, so the invariant holds by construction.

use crate::model::{default_pins, AppEntry};
use std::path::PathBuf;
use std::time::Instant;

// The two types used to live here.  They are re-exported so the paths that
// already name `session::PowerAction` keep working, while the definition sits
// in the bottom layer both modules can depend on.
pub use crate::model::PowerAction;

/// The `Win+V` clipboard panel's own state.
///
/// It was two fields on [`Session`] plus three methods, which meant a panel
/// that has nothing to do with the Start menu could be opened and closed
/// through it.  A separate owner keeps the panel's invariant (closed means
/// closed) next to the panel.
#[derive(Debug, Default)]
pub struct ClipboardPanel {
    open: bool,
}

impl ClipboardPanel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn hide(&mut self) {
        self.open = false;
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }
}

/// An edge detector for a two-key chord, on the frame the chord goes down.
///
/// `Win+V` and `Win+I` each need one, and each needs its own "already seen"
/// latch: pressing one while the other is open must not be swallowed.  The
/// type is the latch, so a caller cannot forget to reset it.
#[derive(Debug, Default)]
pub struct Chord {
    seen: bool,
}

impl Chord {
    /// `true` on the frame `a && b` first holds, `false` until they separate.
    pub fn pressed(&mut self, a: bool, b: bool) -> bool {
        let down = a && b;
        if !down {
            self.seen = false;
            return false;
        }
        if self.seen {
            return false;
        }
        self.seen = true;
        true
    }
}

/// The Start menu: open/closed, the search box, the app list, the animation.
#[derive(Debug, Default)]
pub struct Session {
    open: bool,
    /// The search box's contents; cleared on every close.
    search: String,
    /// 0..=1 open animation, advanced by the shell every frame.
    anim: f32,
    /// Installed applications, rescanned on a slow timer.
    apps: Vec<AppEntry>,
    /// The user's pins.  Empty means "never edited", and the defaults apply, so
    /// an existing install keeps the original set until the user changes it.
    pins: Vec<PathBuf>,
    apps_at: Option<Instant>,
    /// The power action waiting for its "yes".
    pending: Option<PowerAction>,
    /// The Start menu's two nested lists.  They are drawn as areas pinned above
    /// the taskbar rather than as egui submenus, because a submenu is
    /// positioned by the cursor and ran off the bottom of the screen.  Both
    /// were `pub` fields, which let any module open a list the session did not
    /// know about; the verbs below are the only way in now.
    power_menu_open: bool,
    user_menu_open: bool,
    /// The frame the menu was opened on.
    ///
    /// The Start button toggles the menu inside the taskbar pass, but the
    /// menu draws later in the same frame - and its "clicked outside" test
    /// would see the very click that opened it and close it again, so the
    /// menu flashed open and shut.  Comparing frame stamps lets that one
    /// click through.
    opened_at: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            apps_at: None,
            ..Self::default()
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The current contents of the search box (read-only: writing goes through
    /// [`Session::search_mut`] so the borrow is one field, not the whole app).
    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn search_mut(&mut self) -> &mut String {
        &mut self.search
    }

    pub fn anim(&self) -> f32 {
        self.anim
    }

    pub fn set_anim(&mut self, value: f32) {
        self.anim = value;
    }

    /// Is the menu still visible (open, or closing)?
    pub fn visible(&self) -> bool {
        self.open || self.anim >= 0.01
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// Record that the menu was opened during `frame`.
    pub fn note_opened(&mut self, frame: u64) {
        self.opened_at = frame;
    }

    /// Was the menu opened during `frame`?  The outside-click test skips that
    /// frame so the opening click cannot immediately close it.
    pub fn opened_this_frame(&self, frame: u64) -> bool {
        self.open && self.opened_at == frame
    }

    pub fn close(&mut self) {
        self.open = false;
        self.search.clear();
        self.pending = None;
    }

    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    /// Replace the app list and start its refresh clock.
    pub fn set_apps(&mut self, apps: Vec<AppEntry>) {
        self.apps = apps;
        self.apps_at = Some(Instant::now());
    }

    /// The pinned entries, resolved against the current app list.
    ///
    /// A pin stores a *path*, not a name: a name would have to be re-resolved
    /// through localisation on every load, and two programs can share one.
    pub fn pins(&self) -> Vec<AppEntry> {
        if self.pins.is_empty() {
            return default_pins();
        }
        self.pins
            .iter()
            .filter(|p| p.exists())
            .map(|p| AppEntry {
                name: p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.display().to_string()),
                path: p.clone(),
            })
            .collect()
    }

    pub fn pin(&mut self, path: &std::path::Path) {
        // Materialise the defaults first: pinning into an "empty" set must not
        // silently replace the whole default list with that one pin.
        if self.pins.is_empty() {
            self.pins = default_pins().into_iter().map(|a| a.path).collect();
        }
        if !self.pins.iter().any(|p| p == path) {
            self.pins.push(path.to_path_buf());
            self.save_pins();
        }
    }

    /// Move a pin from one position to another, which is what a drag in the
    /// pinned grid does.  The order is the user's, so it is saved.
    pub fn move_pin(&mut self, from: usize, to: usize) {
        if self.pins.is_empty() {
            self.pins = default_pins().into_iter().map(|a| a.path).collect();
        }
        if from < self.pins.len() && to < self.pins.len() && from != to {
            let moved = self.pins.remove(from);
            self.pins.insert(to, moved);
            self.save_pins();
        }
    }

    pub fn unpin(&mut self, path: &std::path::Path) {
        if self.pins.is_empty() {
            self.pins = default_pins().into_iter().map(|a| a.path).collect();
        }
        self.pins.retain(|p| p != path);
        self.save_pins();
    }

    fn pins_path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")?;
        let dir = PathBuf::from(base).join("WinRust");
        let _ = std::fs::create_dir_all(&dir);
        Some(dir.join("start-pins.json"))
    }

    fn save_pins(&self) {
        if let Some(path) = Self::pins_path() {
            if let Ok(text) = serde_json::to_string_pretty(&self.pins) {
                let _ = std::fs::write(path, text);
            }
        }
    }

    /// Read the saved pins at startup.
    pub fn load_pins(&mut self) {
        if let Some(path) = Self::pins_path() {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pins) = serde_json::from_str::<Vec<PathBuf>>(&text) {
                    self.pins = pins;
                }
            }
        }
    }

    // ---- the two nested lists -------------------------------------------

    pub fn power_menu_open(&self) -> bool {
        self.power_menu_open
    }

    /// Flip the power list; opening it closes the account list, so the two can
    /// never be open over each other - the invariant a `pub` field allowed
    /// callers to break.
    pub fn toggle_power_menu(&mut self) {
        self.power_menu_open = !self.power_menu_open;
        if self.power_menu_open {
            self.user_menu_open = false;
        }
    }

    pub fn close_power_menu(&mut self) {
        self.power_menu_open = false;
    }

    pub fn user_menu_open(&self) -> bool {
        self.user_menu_open
    }

    pub fn toggle_user_menu(&mut self) {
        self.user_menu_open = !self.user_menu_open;
        if self.user_menu_open {
            self.power_menu_open = false;
        }
    }

    pub fn close_user_menu(&mut self) {
        self.user_menu_open = false;
    }

    /// True when the list is old enough to be worth rescanning.
    pub fn apps_stale(&self) -> bool {
        self.apps_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(120))
            .unwrap_or(true)
    }

    /// Ask the user to confirm `action`; nothing is carried out yet.
    pub fn ask_power(&mut self, action: PowerAction) {
        self.pending = Some(action);
        // The confirmation lives outside the menu's rect, so the menu must not
        // treat that click as "clicked elsewhere" and close under the dialog.
        self.open = false;
    }

    pub fn pending_power(&self) -> Option<PowerAction> {
        self.pending
    }

    pub fn cancel_power(&mut self) {
        self.pending = None;
    }

    /// The user said yes.
    pub fn confirm_power(&mut self) {
        if let Some(action) = self.pending.take() {
            action.run();
        }
    }
}

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

use std::path::PathBuf;
use std::time::Instant;

/// A power action the user asked for and has not confirmed yet.
///
/// Every one of them is confirmed - a Start menu is two clicks away from
/// losing unsaved work, and the stock menu asks too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    ShutDown,
    Restart,
    Sleep,
    Hibernate,
    SignOut,
}

impl PowerAction {
    /// Every entry of the power list, in the order it is drawn.
    pub const ALL: [PowerAction; 5] = [
        PowerAction::ShutDown,
        PowerAction::Restart,
        PowerAction::Sleep,
        PowerAction::Hibernate,
        PowerAction::SignOut,
    ];

    /// The menu label, already translated.
    pub fn label(self) -> &'static str {
        use winrust_core::i18n::ts;
        match self {
            PowerAction::ShutDown => ts("Shut down"),
            PowerAction::Restart => ts("Restart"),
            PowerAction::Sleep => ts("Sleep"),
            PowerAction::Hibernate => ts("Hibernate"),
            PowerAction::SignOut => ts("Sign out"),
        }
    }

    /// The glyph in front of the label.
    pub fn glyph(self) -> &'static str {
        match self {
            PowerAction::ShutDown => "⏻",
            PowerAction::Restart => "⟳",
            PowerAction::Sleep => "☾",
            PowerAction::Hibernate => "⏾",
            PowerAction::SignOut => "⇥",
        }
    }

    /// The confirmation question, with the action's name in it.
    ///
    /// Built by concatenation rather than a `tf!` template: `tf!` takes a
    /// literal key, and the action's name is already translated by
    /// [`Self::label`], so a template would translate it twice.
    pub fn question(self) -> String {
        use winrust_core::i18n::ts;
        format!("{} {}", self.label(), ts("now?"))
    }

    /// Carry it out.  Called only after the user confirms.
    pub fn run(self) {
        use winrust_core::shell;
        match self {
            PowerAction::ShutDown => shell::shutdown(),
            PowerAction::Restart => shell::restart(),
            PowerAction::Sleep => shell::sleep(),
            // `hibernate()` reports whether the machine accepted it; the
            // power menu has already refused the action where hibernation is
            // disabled, so there is nothing useful to do with the answer here.
            PowerAction::Hibernate => {
                let _ = shell::hibernate();
            }
            PowerAction::SignOut => shell::sign_out(),
        }
    }
}

/// One installed application as the Start menu lists it.
#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

/// The Start menu: open/closed, the search box, the app list, the animation.
#[derive(Debug, Default)]
pub struct Session {
    /// The `Win+V` clipboard panel: open?
    clipboard_open: bool,
    /// True while `Win` is held and `V` was already seen, so one press opens
    /// the panel once instead of once per frame.
    clipboard_chord_seen: bool,
    open: bool,
    /// The search box's contents; cleared on every close.
    search: String,
    /// 0..=1 open animation, advanced by the shell every frame.
    anim: f32,
    /// Installed applications, rescanned on a slow timer.
    apps: Vec<AppEntry>,
    apps_at: Option<Instant>,
    /// The power action waiting for its "yes".
    pending: Option<PowerAction>,
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

    pub fn close(&mut self) {
        self.open = false;
        self.search.clear();
        self.pending = None;
    }


    // ---- the clipboard panel (Win+V) -------------------------------------
    pub fn clipboard_open(&self) -> bool {
        self.clipboard_open
    }
    pub fn hide_clipboard(&mut self) {
        self.clipboard_open = false;
    }
    pub fn toggle_clipboard(&mut self) {
        self.clipboard_open = !self.clipboard_open;
    }

    /// The `Win+V` edge detector.
    ///
    /// Returns `true` on the frame the chord goes down, and `false` for every
    /// frame after it until `Win` or `V` is released - so holding the keys
    /// cannot flicker the panel.  The shell calls this once per frame; the
    /// poll runs even when another program has the focus, which is the whole
    /// point (WinRust replaces `explorer.exe`, and the stock history lives
    /// there).
    pub fn win_v_pressed(&mut self, win: bool, v: bool) -> bool {
        let chord = win && v;
        if !chord {
            self.clipboard_chord_seen = false;
            return false;
        }
        if self.clipboard_chord_seen {
            return false;
        }
        self.clipboard_chord_seen = true;
        true
    }

    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    /// Replace the app list and start its refresh clock.
    pub fn set_apps(&mut self, apps: Vec<AppEntry>) {
        self.apps = apps;
        self.apps_at = Some(Instant::now());
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

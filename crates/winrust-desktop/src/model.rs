//! Types the Start menu and the session both need.
//!
//! This is the bottom layer of the desktop crate: it depends on nothing but
//! `winrust-core`.  `session` (the state owner) and `start` (the view) used to
//! reference each other - `start` for the two types below, `session` for
//! [`default_pins`] - which made the pair one module in two files as far as
//! the compiler was concerned.  Moving the shared vocabulary here lets both
//! depend on it downwards instead of on each other sideways.

use std::path::PathBuf;
use winrust_core::t;

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

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

/// The applications the pinned grid shows.
///
/// Windows pins what its vendors install; a bare install has no pin database
/// we can read without COM, so the shell pins its own programs plus the
/// handful of things Windows always ships.  The point of the grid is to be
/// clickable without reading, so only paths that exist are kept.
pub(crate) fn default_pins() -> Vec<AppEntry> {
    let mut pins: Vec<AppEntry> = Vec::new();
    for (name, exe) in [
        ("WinRust Explorer", "winrust-explorer.exe"),
        ("WinRust Task Manager", "winrust-taskmgr.exe"),
        ("WinRust Services", "winrust-services.exe"),
        ("WinRust System Control", "winrust-sysctl.exe"),
        ("WinRust Launcher", "WinRust.exe"),
        ("Notepad", "notepad.exe"),
        ("Paint", "mspaint.exe"),
        ("Calculator", "calc.exe"),
        ("Command Prompt", "cmd.exe"),
        ("Control Panel", "control.exe"),
        ("Task Manager", "taskmgr.exe"),
        ("File Explorer", "explorer.exe"),
    ] {
        if let Some(path) = winrust_core::install::find_binary(exe) {
            pins.push(AppEntry {
                name: t(name),
                path,
            });
        }
    }
    pins
}

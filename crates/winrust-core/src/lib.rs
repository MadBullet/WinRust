//! `winrust-core` — the shared bottom layer of the WinRust project.
//!
//! Every wrapper in this crate is a thin, documented, ownership-safe facade
//! over a raw Win32 API.  The goal is that UI crates never touch
//! `windows-sys` directly, never leak handles, and never have to reason about
//! the difference between a `BOOL` and a `bool`.
//!
//! Modules
//! -------\n//! * [`wide`]        — UTF-16 buffer helpers
//! * [`error`]       — `WinError`, the single error type used across the project
//! * [`handle`]      — RAII ownership for kernel handles
//! * [`gpu`]         — per-adapter GPU utilisation through PDH
//! * [`i18n`]        — the shared English/Russian translation layer
//! * [`process`]     — process enumeration, inspection, termination
//! * [`system`]      — memory, uptime, computer name, OS build
//! * [`disk`]        — volumes, free space, file-system walk / file operations
//! * [`net`]         — TCP/UDP connection tables with owning PIDs
//! * [`registry`]    — typed registry access
//! * [`shell`]       — ShellExecute, file icons, known folders, recycle bin
//! * [`window`]      — top-level window list, icons, focus, the `Win` key
//! * [`service`]     — SCM enumeration and control
//! * [`user`]        — token information (user, elevation, integrity)
//! * [`sysinfo_cpu`] — per-process CPU accounting helper
//!
//! Everything is `#[cfg(windows)]`; the crate intentionally does not build on
//! other platforms because it models Windows behaviour exactly.

#![cfg_attr(not(windows), allow(unused))]

#[cfg(not(windows))]
compile_error!("winrust-core targets Windows only. Build it on Windows or in CI.");

pub mod audio;
pub mod disk;
pub mod error;
pub mod fileassoc;
pub mod files;
pub mod gpu;
pub mod handle;
pub mod i18n;
pub mod icon;
pub mod install;
pub mod net;
pub mod process;
pub mod registry;
pub mod restore;
pub mod service;
pub mod shell;
pub mod system;
pub mod thumb;
pub mod user;
pub mod wide;
pub mod window;

pub use error::{Result, WinError};
pub use handle::Handle;
pub use i18n::{t, ts, Lang};

/// Format a Win32 error code into a readable message (falls back to the code).
pub fn format_error(code: u32) -> String {
    error::message_for(code)
}

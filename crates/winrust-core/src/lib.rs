//! `winrust-core` — the shared bottom layer of the WinRust project.
//!
//! Every wrapper in this crate is a thin, documented, ownership-safe facade
//! over a raw Win32 API.  The goal is that UI crates never touch
//! `windows-sys` directly, never leak handles, and never have to reason about
//! the difference between a `BOOL` and a `bool`.
//!
//! Modules
//! -------
//! The crate is a stack, and the modules are grouped so the stack is visible:
//!
//! * leaf      — [`wide`] (UTF-16), [`error`] (`WinError`), [`handle`] (RAII),
//!               [`icon`] (HICON to RGBA)
//! * [`storage`] — [`disk`], [`files`], [`fileassoc`], [`registry`],
//!                 [`restore`], [`thumb`]
//! * [`system`]  — [`system`](self::system::mod@system), [`process`],
//!                 [`service`], [`user`], [`gpu`]
//! * [`shell`]   — ShellExecute, known folders, the recycle bin,
//!                 [`window`]
//! * [`devices`] — [`audio`], [`net`], [`install`]
//! * [`i18n`]    — the shared English/Russian translation layer
//!
//! Each old path still works: `winrust_core::process` is
//! `winrust_core::system::process` behind a `pub use`, so grouping the files
//! changed no call site.
//!
//! Everything is `#[cfg(windows)]`; the crate intentionally does not build on
//! other platforms because it models Windows behaviour exactly.

#![cfg_attr(not(windows), allow(unused))]

#[cfg(not(windows))]
compile_error!("winrust-core targets Windows only. Build it on Windows or in CI.");

// The leaf layer: nothing above depends on anything below it in this list.
pub mod error;
pub mod handle;
pub mod icon;
pub mod wide;

// The groups.  `system.rs` and `shell.rs` are both a group and a module, so
// the group's facade re-exports the module under its own name.
pub mod devices;
pub mod i18n;
pub mod shell;
pub mod storage;
pub mod system;

// The old flat paths, kept so no caller had to move: `winrust_core::disk`
// still resolves, now to `winrust_core::storage::disk`.
pub use devices::{audio, install, net};
pub use shell::window;
pub use storage::{disk, fileassoc, files, registry, restore, thumb};
pub use system::{gpu, process, service, user};

pub use error::{Result, WinError};
pub use handle::Handle;
pub use i18n::{t, ts, Lang};

/// Format a Win32 error code into a readable message (falls back to the code).
pub fn format_error(code: u32) -> String {
    error::message_for(code)
}

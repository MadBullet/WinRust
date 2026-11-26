//! The Windows shell: launching, known folders, the recycle bin - and the
//! window list the taskbar draws.
//!
//! [`mod@shell`] is the ShellExecute side ([`shell::open`], [`shell::execute`],
//! known folders, the recycle bin); [`window`] is the window side;
//! [`clipboard`] is the `Win+V` history.

pub mod clipboard;
pub mod shell;
pub mod window;

pub use self::clipboard::History as ClipboardHistory;

// `winrust_core::shell::open(..)` and the rest keep resolving: the module of
// that name is this group's own file.
pub use self::shell::*;

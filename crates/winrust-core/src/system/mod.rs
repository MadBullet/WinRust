//! The machine the shell runs on: processes, services, accounts, the GPU.
//!
//! These four read the same Windows tables (a process owns a service, a
//! token names a user, a GPU adapter is a driver), so they belong together;
//! each keeps its own file and its own public path.

pub mod gpu;
pub mod process;
pub mod service;
pub mod system;
pub mod user;

// `winrust_core::system::memory_status()` and `winrust_core::system::os_info()`
// keep resolving: the module of that name is this group's own file.
pub use self::system::*;

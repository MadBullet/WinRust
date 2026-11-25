//! Everything about files: drives, listings, file operations, associations,
//! icons and the registry that Windows keeps them in.
//!
//! The registry is here rather than next to `system` because every one of
//! these modules reads it: a ProgID, a restore point, a thumbnail cache.

pub mod disk;
pub mod fileassoc;
pub mod files;
pub mod registry;
pub mod restore;
pub mod thumb;

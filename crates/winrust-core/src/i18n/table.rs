//! The Russian translation table.
//!
//! Layout: one slice per component plus a shared `common` block.  The English
//! string is the key; `tr!("Cancel")` returns "Отмена" when the language is
//! Russian.  Placeholders (`{}`) must appear in the same order in both strings
//! — a unit test enforces that.

pub mod common;
pub mod explorer;
pub mod launcher;
pub mod services;
pub mod sysctl;
pub mod taskmgr;

/// All component tables, in lookup order (first entry for a key wins).
pub static TABLES: &[&[(&str, &str)]] = &[
    common::ENTRIES,
    taskmgr::ENTRIES,
    services::ENTRIES,
    explorer::ENTRIES,
    sysctl::ENTRIES,
    launcher::ENTRIES,
];

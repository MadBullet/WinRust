//! `winrust-ui` — shared look for the WinRust programs.
//!
//! * [`theme`] — the common dark palette, the egui visuals and the optional
//!   window transparency / blur plumbing.
//!
//! Localisation lives in `winrust_core::i18n` so even non-UI crates (service
//! names, priority labels) can translate their own strings.

pub mod theme;

pub use winrust_core::i18n::Appearance;

//! The single language setting shared by every WinRust component.
//!
//! The launcher writes it; every other program reads it at start-up.  It lives
//! in `HKCU\SOFTWARE\WinRust` as a `REG_SZ` value, so no elevation is needed and
//! a portable (uninstalled) copy works exactly the same.

use winrust_core::registry::{self, Root};

const KEY: &str = r"SOFTWARE\WinRust";

/// The languages WinRust speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// English is the default, as requested.
    #[default]
    En,
    Ru,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    /// Name in its own language, as a language picker should show it.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        let c = code.trim().to_ascii_lowercase();
        match c.as_str() {
            "en" | "en-us" | "en-gb" | "english" => Some(Lang::En),
            "ru" | "ru-ru" | "russian" | "русский" => Some(Lang::Ru),
            _ => None,
        }
    }

    /// `true` when the given code selects this language (`ru-RU` counts as `ru`).
    pub fn matches(self, code: &str) -> bool {
        matches!(Lang::parse(code), Some(l) if l == self)
    }

    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// The stored language, falling back to English.
pub fn load() -> Lang {
    registry::read_user_string(KEY, "Lang")
        .and_then(|v| Lang::parse(&v))
        .unwrap_or_default()
}

/// Persist the language so every component picks it up on its next start.
pub fn save(lang: Lang) -> winrust_core::Result<()> {
    let key = registry::open_for_write(Root::CurrentUser, KEY)?;
    key.set_string("Lang", lang.code())
}

/// Read-only helper: `true` when the stored language is Russian.
pub fn is_ru() -> bool {
    load() == Lang::Ru
}

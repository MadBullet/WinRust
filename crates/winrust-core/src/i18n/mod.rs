//! Localisation shared by every WinRust program.
//!
//! The English text is the lookup key, so a missing translation degrades to
//! English instead of showing a placeholder id.  The chosen language lives in
//! `HKCU\SOFTWARE\WinRust` and is written by the launcher, which is how the
//! launcher's language selector reaches the other five programs.

pub mod lang;
pub mod settings;
pub mod shorten;

pub use lang::Lang;
pub use settings::{Appearance, Theme};
pub use shorten::{end, head, middle, wrap};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// Cached language, so a translation never touches the registry mid-frame.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Read the language from the registry and cache it. Every component calls
/// this once at start-up; the launcher calls it after the user switches.
pub fn init() -> Lang {
    let l = lang::load();
    set_lang(l);
    l
}

/// Cache a language without writing it to the registry.
pub fn set_lang(l: Lang) {
    CURRENT.store(
        match l {
            Lang::En => 0,
            Lang::Ru => 1,
        },
        Ordering::Relaxed,
    );
}

/// The language currently in effect.
pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

/// Serialises the tests that change the process-wide language.
///
/// `CURRENT` is a `static` shared by the whole test binary, and `cargo test`
/// runs tests on several threads: an i18n test that switches to Russian can
/// run *while* an unrelated test formats a disk size or a status line, which
/// is exactly how `storage::disk::tests::size_formatting` failed in CI while
/// passing locally.  Any test that calls [`set_lang`] - or asserts on
/// translated output - takes this lock first.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A test that panicked while holding the lock leaves it poisoned; the
    // language is reset by every lock holder, so recovering is correct here.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `true` when the UI should speak Russian.
pub fn ru() -> bool {
    current() == Lang::Ru
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        for t in crate::i18n::table::TABLES {
            for (en, ru) in t.iter() {
                m.entry(*en).or_insert(*ru);
            }
        }
        m
    })
}

/// Translate one English UI string.
pub fn t(en: &'static str) -> String {
    if ru() {
        match table().get(en) {
            Some(r) => (*r).to_string(),
            None => en.to_string(),
        }
    } else {
        en.to_string()
    }
}

/// Translate and return a borrowed static string when possible.
pub fn ts(en: &'static str) -> &'static str {
    if ru() {
        table().get(en).copied().unwrap_or(en)
    } else {
        en
    }
}

/// Resolve the indirect strings the registry uses for display names.
///
/// `FriendlyTypeName` is almost never a literal: it is `@%SystemRoot%\system32\
/// shell32.dll,-12345`, a resource id inside a DLL.  Windows asks
/// `SHLoadIndirectString` (shlwapi.dll) for the text, so we do too - a raw
/// registry string showing `@...,-123` in the UI is a bug.
///
/// A plain literal is returned unchanged, which is what most third-party keys
/// (SAP, WinRAR, ...) store.
pub fn localize_resource_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with('@') && !trimmed.contains(",-") {
        return trimmed.to_string();
    }
    if let Some(text) = unsafe { load_indirect(trimmed) } {
        if !text.is_empty() {
            return text;
        }
    }
    // `@path,-123`: a bare file name beats a resource id in the menu.
    if let Some((path, _)) = trimmed.trim_start_matches('@').split_once(',') {
        if let Some(name) = path.rsplit(['\\', '/']).next() {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    trimmed.to_string()
}

/// `SHLoadIndirectString` from shlwapi.dll, resolved at run time.
///
/// The workspace never links a DLL by hand (no `#[link]` anywhere), so this
/// follows the same rule: `LoadLibrary`/`GetProcAddress` instead of a new
/// build-time dependency.  Returns `None` when the library or the export is
/// missing, and the caller falls back to a readable name.
///
/// # Safety
/// Called only from [`localize_resource_string`] with a NUL-terminated buffer.
unsafe fn load_indirect(source: &str) -> Option<String> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    #[allow(non_snake_case)]
    type SHLoadIndirectStringFn = unsafe extern "system" fn(
        pszsource: *const u16,
        pszoutbuf: *mut u16,
        cchoutbuf: u32,
        ppvreserved: *const *const core::ffi::c_void,
    ) -> i32;

    let lib_name = crate::wide::str_to_wide("shlwapi.dll");
    let lib = LoadLibraryW(lib_name.as_ptr());
    if lib.is_null() {
        return None;
    }
    // The library is deliberately never freed: Windows keeps a reference for the
    // process lifetime, and a leaked HMODULE is cheaper than a use-after-free.
    let Ok(proc_name) = std::ffi::CString::new("SHLoadIndirectString") else {
        return None;
    };
    // `FARPROC` is `Option<unsafe extern "system" fn() -> isize>` in
    // windows-sys 0.59, not a bare pointer, so it is unwrapped first.
    let addr = GetProcAddress(lib, proc_name.as_ptr() as *const u8);
    let load: SHLoadIndirectStringFn = std::mem::transmute(addr?);

    let src = crate::wide::str_to_wide(source);
    let mut out = [0u16; 512];
    let hr = load(
        src.as_ptr(),
        out.as_mut_ptr(),
        out.len() as u32,
        std::ptr::null(),
    );
    if hr < 0 {
        return None;
    }
    Some(crate::wide::from_wide(&out, out.len()))
}

/// Fill `{}` placeholders in order.  Extra arguments are appended in
/// parentheses so a mismatched call is visible rather than silent.
pub fn fill(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len() + 16 * args.len());
    let mut rest = template;
    let mut used = 0usize;
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        match args.get(used) {
            Some(a) => out.push_str(a),
            None => out.push_str("{}"),
        }
        used += 1;
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    if used < args.len() {
        out.push_str(" (");
        out.push_str(&args[used..].join(", "));
        out.push(')');
    }
    out
}

/// Used by [`tf!`] to turn any displayable value into text.
pub fn arg(v: impl std::fmt::Display) -> String {
    v.to_string()
}

/// `tr!("Text")` — localised static text.
#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::t($s)
    };
}

/// `tf!("Suspended {} threads", n)` — localised template plus runtime values.
#[macro_export]
macro_rules! tf {
    ($s:literal $(, $arg:expr)* $(,)?) => {{
        let __args: ::std::vec::Vec<::std::string::String> =
            ::std::vec![$($crate::i18n::arg($arg)),*];
        $crate::i18n::fill(&$crate::i18n::t($s), &__args)
    }};
}

pub mod table;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_identity() {
        let _lock = test_lock();
        set_lang(Lang::En);
        assert_eq!(t("Cancel"), "Cancel");
    }

    #[test]
    fn russian_translates_known_and_keeps_unknown() {
        let _lock = test_lock();
        set_lang(Lang::Ru);
        assert_eq!(t("Cancel"), "Отмена");
        assert_eq!(
            t("Some string nobody translated"),
            "Some string nobody translated"
        );
        set_lang(Lang::En);
    }

    #[test]
    fn fill_replaces_in_order() {
        assert_eq!(
            fill("{} threads of {}", &["12".into(), "chrome.exe".into()]),
            "12 threads of chrome.exe"
        );
        assert_eq!(fill("no placeholders", &[]), "no placeholders");
    }

    #[test]
    fn every_translation_has_the_same_placeholders() {
        for t in table::TABLES {
            for (en, ru) in t.iter() {
                assert_eq!(
                    en.matches("{}").count(),
                    ru.matches("{}").count(),
                    "placeholder mismatch: {en} -> {ru}"
                );
            }
        }
    }

    #[test]
    fn no_duplicate_source_strings() {
        let mut seen = std::collections::HashSet::new();
        for t in table::TABLES {
            for (en, _) in t.iter() {
                assert!(seen.insert(*en), "duplicate translation key: {en}");
            }
        }
    }
}

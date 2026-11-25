//! What the shell can create and what it can open, read from the registry.
//!
//! Windows builds both the "New" submenu and the "Open with" list from the
//! *registered* state of the machine, not from a hard-coded list: a program
//! that installs itself adds a `ShellNew` sub-key under `HKEY_CLASSES_ROOT\
//! .<ext>`, and an "Open with" entry that is registered under
//! `...\Applications\<exe>\shell\open`.  This module reads exactly those places,
//! so the menu follows what is actually installed:
//!
//! * a machine with Word installed shows "Word Document",
//! * a machine without it shows nothing for `.docx`,
//! * WinRAR adds ".rar" to "New" the moment it registers itself.
//!
//! The registry is 64-bit view (`KEY_WOW64_64KEY`), which is what Explorer
//! uses for the shell namespace.

use crate::i18n::ts;
use crate::registry::{Key, Root};

/// One entry of the "New" submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
    /// The submenu label, from the shell's own `FriendlyTypeName`/`@dll,-id`
    /// resolution, or the extension when nothing better is registered.
    pub label: String,
    /// Extension without the dot.
    pub extension: String,
}

/// One program that can open a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWith {
    /// Program name shown in the menu.
    pub name: String,
    /// Command line, with `%1` for the file.
    pub command: String,
}

/// Extensions Windows never offers in "New".
const SKIP_EXT: &[&str] = &[
    "exe", "dll", "sys", "msi", "msp", "cpl", "scr", "inf", "ico",
];

/// The "New" submenu: every extension that registers a `ShellNew` handler.
///
/// `dir` decides which entries apply to folders (Windows hides file templates
/// when the current folder is read-only, but the plain "Folder" and "Shortcut"
/// entries always come from the caller).
pub fn new_items() -> Vec<NewItem> {
    let mut out: Vec<NewItem> = Vec::new();
    let Ok(root) = Key::open(Root::LocalMachine, "SOFTWARE\\Classes") else {
        return out;
    };
    for name in root.subkeys() {
        if !name.starts_with('.') || name.len() < 2 {
            continue;
        }
        let ext = name[1..].to_ascii_lowercase();
        if SKIP_EXT.contains(&ext.as_str()) {
            continue;
        }
        let Ok(key) = Key::open(
            Root::LocalMachine,
            &format!("SOFTWARE\\Classes\\{name}\\ShellNew"),
        ) else {
            continue;
        };
        // A `ShellNew` key can exist for other reasons (e.g. only a
        // `Command`); Windows itself requires one of the value forms.
        let has_template = key.string("FileName").is_some()
            || key.string("NullFile").is_some()
            || key.string("Directory").is_some()
            || key.string("Data").is_some()
            || key.string("Command").is_some();
        if !has_template {
            continue;
        }
        out.push(NewItem {
            label: friendly_type(&ext),
            extension: ext,
        });
    }
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out.dedup_by(|a, b| a.extension == b.extension);
    out
}

/// Create the empty file a `ShellNew` entry describes, or `None` when the
/// extension has no usable template (the caller falls back to an empty file).
pub fn new_template(extension: &str) -> Option<std::path::PathBuf> {
    let key = Key::open(
        Root::LocalMachine,
        &format!("SOFTWARE\\Classes\\.{extension}\\ShellNew"),
    )
    .ok()?;
    let dir = key.string("Directory")?;
    let file = key.string("FileName")?;
    let path = std::path::Path::new(&dir).join(file);
    path.exists().then_some(path)
}

/// The human name of an extension: `FriendlyTypeName`, then the bare
/// description, then a dotted upper-case fallback.
fn friendly_type(ext: &str) -> String {
    if ext.eq_ignore_ascii_case("txt") {
        return ts("Text Document").to_string();
    }
    if ext.eq_ignore_ascii_case("bmp") {
        return ts("Bitmap image").to_string();
    }
    if ext.eq_ignore_ascii_case("zip") {
        return ts("Compressed (zipped) Folder").to_string();
    }
    if Key::open(Root::LocalMachine, "SOFTWARE\\Classes").is_err() {
        return ext.to_ascii_uppercase();
    }
    // `.<ext>\(Default)` names the ProgID, whose own default value is the name.
    if let Ok(key) = Key::open(Root::LocalMachine, &format!("SOFTWARE\\Classes\\.{ext}")) {
        if let Some(progid) = key.string("") {
            if let Ok(pkg) = Key::open(Root::LocalMachine, &format!("SOFTWARE\\Classes\\{progid}"))
            {
                if let Some(name) = pkg.string("") {
                    if !name.is_empty() {
                        return name;
                    }
                }
                if let Some(name) = pkg.string("FriendlyTypeName") {
                    return crate::i18n::localize_resource_string(&name);
                }
            }
        }
        if let Some(name) = key.string("FriendlyTypeName") {
            return crate::i18n::localize_resource_string(&name);
        }
    }
    ext.to_ascii_uppercase()
}

/// Programs that can open `path`, the way Explorer's "Open with" submenu
/// collects them: every `Applications\*.exe\shell\open\command` that registers
/// the extension, plus the extension's own `OpenWithList`.
pub fn open_with(path: &std::path::Path) -> Vec<OpenWith> {
    let Some(ext) = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
    else {
        return Vec::new();
    };
    let mut out: Vec<OpenWith> = Vec::new();

    // 1. `HKCR\Applications\foo.exe\SupportedTypes\.ext` - the per-program opt-in.
    if let Ok(apps) = Key::open(Root::LocalMachine, "SOFTWARE\\Classes\\Applications") {
        for exe in apps.subkeys() {
            let base = format!("SOFTWARE\\Classes\\Applications\\{exe}");
            let Ok(app) = Key::open(Root::LocalMachine, &base) else {
                continue;
            };
            let supported = app
                .subkeys()
                .iter()
                .any(|k| k.eq_ignore_ascii_case(&format!(".{ext}")));
            if !supported {
                continue;
            }
            let Ok(cmd_key) =
                Key::open(Root::LocalMachine, &format!("{base}\\shell\\open\\command"))
            else {
                continue;
            };
            let Some(command) = cmd_key.string("") else {
                continue;
            };
            out.push(OpenWith {
                name: exe_name(&exe),
                command,
            });
        }
    }

    // 2. `HKCR\.ext\OpenWithList` names executables without their own entry.
    if let Ok(key) = Key::open(
        Root::LocalMachine,
        &format!("SOFTWARE\\Classes\\.{ext}\\OpenWithList"),
    ) {
        for name in key.subkeys() {
            if !name.to_ascii_lowercase().ends_with(".exe") {
                continue;
            }
            if let Some(found) = command_for_exe(&name) {
                out.push(found);
            }
        }
        for value in ["a", "b", "c", "d", "e", "f"] {
            if let Some(name) = key.string(value) {
                if let Some(found) = command_for_exe(&name) {
                    out.push(found);
                }
            }
        }
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// The `shell\open\command` of a registered executable, by name.
fn command_for_exe(exe: &str) -> Option<OpenWith> {
    let key = Key::open(
        Root::LocalMachine,
        &format!("SOFTWARE\\Classes\\Applications\\{exe}\\shell\\open\\command"),
    )
    .ok()?;
    Some(OpenWith {
        name: exe_name(exe),
        command: key.string("")?,
    })
}

/// `notepad.exe` -> `Notepad`, without the extension.
fn exe_name(exe: &str) -> String {
    let stem = exe.rsplit_once('.').map(|(s, _)| s).unwrap_or(exe);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => exe.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_are_tidied() {
        assert_eq!(exe_name("notepad.exe"), "Notepad");
        assert_eq!(exe_name("WINWORD.EXE"), "WINWORD");
    }

    #[test]
    fn slash_free_extensions_only() {
        // A guard against the registry returning a key with a dot but no name.
        for item in new_items() {
            assert!(!item.extension.is_empty());
            assert!(!item.extension.starts_with('.'));
        }
    }
}

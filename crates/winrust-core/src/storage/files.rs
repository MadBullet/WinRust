//! File operations the shell shares between programs.
//!
//! The desktop and the Explorer window both delete, create and rename files,
//! and both had their own copy of the naming rules and the confirm-then-act
//! loop.  Two copies drift: the desktop learned `Shift+Delete` later than the
//! Explorer, and the "New folder (2)" numbering existed twice with different
//! fallbacks.  This module owns the rules once.
//!
//! The dialog *UI* stays in each program (egui widgets belong to the UI crate);
//! what is shared here is the part that touches the disk and the Recycle Bin.

use std::path::{Path, PathBuf};

/// Outcome of a confirmed delete, so callers can report it in their status bar.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeleteReport {
    /// How many paths were handled.
    pub done: usize,
    /// `name: reason` for every path that failed.
    pub errors: Vec<String>,
}

impl DeleteReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Move `paths` to the Recycle Bin, or delete them for good.
///
/// `permanent` is what `Shift+Delete` and the "Delete permanently" menu item
/// mean.  A virtual entry (This PC, the Recycle Bin) has no path and is refused
/// here rather than silently skipped by the caller.
pub fn delete(paths: &[PathBuf], permanent: bool) -> DeleteReport {
    let mut report = DeleteReport::default();
    for path in paths {
        if !path.exists() {
            report
                .errors
                .push(format!("{}: not found", crate::disk::file_name_of(path)));
            continue;
        }
        let result = if permanent {
            crate::disk::delete_permanent(path)
        } else {
            crate::shell::recycle(std::slice::from_ref(path))
        };
        match result {
            Ok(()) => report.done += 1,
            Err(e) => report
                .errors
                .push(format!("{}: {e}", crate::disk::file_name_of(path))),
        }
    }
    report
}

/// A name that does not exist yet in `dir`: `base`, `base (2)`, `base (3)`...
///
/// `extension` is `None` for folders.  This is Explorer's own numbering, which
/// is why the second copy is always `(2)` and not `(1)`.
pub fn unique_name(dir: &Path, base: &str, extension: Option<&str>) -> String {
    let build = |n: usize| match (n, extension) {
        (1, Some(ext)) => format!("{base}.{ext}"),
        (1, None) => base.to_string(),
        (n, Some(ext)) => format!("{base} ({n}).{ext}"),
        (n, None) => format!("{base} ({n})"),
    };
    let mut n = 1usize;
    loop {
        let candidate = build(n);
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
        // A directory with tens of thousands of copies is a bug, not a user.
        if n > 9999 {
            return candidate;
        }
    }
}

/// Create a new file from the shell's `ShellNew` template when one exists,
/// otherwise an empty file.  Returns the path it created.
pub fn create_file(dir: &Path, extension: &str, base_name: &str) -> std::io::Result<PathBuf> {
    let ext = extension.trim_start_matches('.');
    let name = unique_name(dir, base_name, Some(ext));
    let path = dir.join(name);
    match crate::fileassoc::new_template(ext) {
        Some(template) => std::fs::copy(template, &path).map(|_| path),
        None => std::fs::File::create(&path).map(|_| path),
    }
}

/// Create a new folder with Explorer's numbering.
pub fn create_folder(dir: &Path, base_name: &str) -> std::io::Result<PathBuf> {
    let name = unique_name(dir, base_name, None);
    let path = dir.join(name);
    std::fs::create_dir(&path).map(|_| path)
}

#[cfg(test)]
mod tests {
    use crate::storage::files::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("winrust-files-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn numbering_starts_at_two() {
        let dir = temp_dir("numbering");
        assert_eq!(unique_name(&dir, "New folder", None), "New folder");
        std::fs::create_dir(dir.join("New folder")).unwrap();
        assert_eq!(unique_name(&dir, "New folder", None), "New folder (2)");
        std::fs::create_dir(dir.join("New folder (2)")).unwrap();
        assert_eq!(unique_name(&dir, "New folder", None), "New folder (3)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_keep_their_extension() {
        let dir = temp_dir("ext");
        assert_eq!(unique_name(&dir, "New file", Some("txt")), "New file.txt");
        std::fs::write(dir.join("New file.txt"), b"x").unwrap();
        assert_eq!(
            unique_name(&dir, "New file", Some("txt")),
            "New file (2).txt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_missing_path_is_reported_not_panicked() {
        let report = delete(&[PathBuf::from("C:\\definitely\\not\\here")], true);
        assert_eq!(report.done, 0);
        assert_eq!(report.errors.len(), 1);
    }
}

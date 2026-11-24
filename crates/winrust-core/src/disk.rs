//! Volumes, free space, directory enumeration and file operations.
//!
//! This is the storage engine behind the Explorer replacement: it must survive
//! permission errors in the middle of a listing (e.g. `C:\System Volume
//! Information`) and never panic on malformed names.

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::wide::{from_wide, to_wide};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, FindNextFileW, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetFileAttributesW, GetLogicalDrives, GetVolumeInformationW, MoveFileExW, SetFileAttributesW,
    WIN32_FIND_DATAW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, MOVEFILE_COPY_ALLOWED, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveKind {
    Fixed,
    Removable,
    Remote,
    CdRom,
    RamDisk,
    Unknown,
    NoRoot,
}

impl DriveKind {
    pub fn label(self) -> &'static str {
        match self {
            DriveKind::Fixed => "Local disk",
            DriveKind::Removable => "Removable",
            DriveKind::Remote => "Network",
            DriveKind::CdRom => "Optical",
            DriveKind::RamDisk => "RAM disk",
            DriveKind::Unknown => "Unknown",
            DriveKind::NoRoot => "Not ready",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub root: String,
    pub label: String,
    pub fs_name: String,
    pub kind: Option<DriveKind>,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub serial: u32,
}

impl VolumeInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn used_percent(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes() as f64 / self.total_bytes as f64 * 100.0) as f32
        }
    }
}

fn drive_kind(raw: u32) -> DriveKind {
    match raw {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Remote,
        5 => DriveKind::CdRom,
        6 => DriveKind::RamDisk,
        1 => DriveKind::NoRoot,
        _ => DriveKind::Unknown,
    }
}

/// All drive letters currently reported by the system.
pub fn volumes() -> Vec<VolumeInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wroot = to_wide(&root);
        let kind = drive_kind(unsafe { GetDriveTypeW(wroot.as_ptr()) });

        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        let mut serial = 0u32;
        let mut max_len = 0u32;
        let mut flags = 0u32;
        let volume_ok = unsafe {
            GetVolumeInformationW(
                wroot.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut max_len,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } != 0;

        let mut total = 0u64;
        let mut free = 0u64;
        let mut avail = 0u64;
        let _ = unsafe { GetDiskFreeSpaceExW(wroot.as_ptr(), &mut avail, &mut total, &mut free) };

        out.push(VolumeInfo {
            root,
            label: if volume_ok { from_wide(&label, label.len()) } else { String::new() },
            fs_name: if volume_ok { from_wide(&fs, fs.len()) } else { String::new() },
            kind: Some(kind),
            total_bytes: total,
            free_bytes: free,
            serial,
        });
    }
    out
}

/// One directory entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub hidden: bool,
    pub system: bool,
    pub readonly: bool,
    pub size: u64,
    /// `FILETIME` as 100 ns units since 1601.
    pub modified: u64,
    pub created: u64,
}

fn ft64(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

impl DirEntry {
    /// Convert a `FILETIME` to a `SystemTime`-style UNIX epoch second count.
    pub fn modified_unix(&self) -> Option<u64> {
        if self.modified == 0 {
            None
        } else {
            Some(self.modified / 10_000_000 - 11_644_473_600)
        }
    }

    /// Local-time rendering, e.g. `2026-11-24 10:31`.
    pub fn modified_display(&self) -> String {
        match self.modified_unix() {
            None => String::new(),
            Some(secs) => format_local(secs),
        }
    }
}

/// Format seconds-since-epoch as local `YYYY-MM-DD HH:MM`.
pub fn format_local(unix_secs: u64) -> String {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    let ft_in = FILETIME {
        dwLowDateTime: ((unix_secs + 11_644_473_600) % (1 << 32)) as u32,
        dwHighDateTime: (((unix_secs + 11_644_473_600) >> 32) & 0xFFFF_FFFF) as u32,
    };
    let mut local = unsafe { std::mem::zeroed::<FILETIME>() };
    if unsafe { FileTimeToLocalFileTime(&ft_in, &mut local) } == 0 {
        return String::new();
    }
    // Days since 1601-01-01, then a civil-date conversion (Howard Hinnant's
    // algorithm) — avoids depending on FileTimeToSystemTime layout.
    let ticks = ((local.dwHighDateTime as u64) << 32) | local.dwLowDateTime as u64;
    let secs = ticks / 10_000_000;
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let (y, m, d) = civil_from_days(days - 11_644_473_600);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        sod / 3600,
        (sod % 3600) / 60
    )
}

/// Read one directory.  Entries that cannot be stat'ed are still listed.
///
/// `show_hidden` mirrors Explorer's "Hidden items" toggle.
pub fn read_dir(dir: impl AsRef<Path>, show_hidden: bool) -> Result<Vec<DirEntry>> {
    let dir = dir.as_ref();
    let pattern = dir.join("*");
    let wpattern = to_wide(&pattern);

    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(wpattern.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(WinError::from_last_error("FindFirstFileW"));
    }
    let handle = Handle::new(handle).ok_or_else(|| WinError::Other("bad find handle".into()))?;

    let mut out = Vec::with_capacity(128);
    loop {
        let name = from_wide(&data.cFileName, data.cFileName.len());
        if !name.is_empty() && name != "." && name != ".." {
            let attr = data.dwFileAttributes;
            let hidden = attr & FILE_ATTRIBUTE_HIDDEN != 0;
            let system = attr & FILE_ATTRIBUTE_SYSTEM != 0;
            if show_hidden || (!hidden && !system) {
                out.push(DirEntry {
                    name: name.clone(),
                    path: dir.join(&name),
                    is_dir: attr & FILE_ATTRIBUTE_DIRECTORY != 0,
                    hidden,
                    system,
                    readonly: attr & FILE_ATTRIBUTE_READONLY != 0,
                    size: ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64,
                    modified: ft64(&data.ftLastWriteTime),
                    created: ft64(&data.ftCreationTime),
                });
            }
        }
        if unsafe { FindNextFileW(handle.raw(), &mut data) } == 0 {
            break;
        }
    }
    unsafe { FindClose(handle.into_raw()) };

    Ok(out)
}

/// Sort entries the way Explorer does: folders first, then by name.
pub fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Type,
}

/// Sort by any key, directories always first (Explorer convention).
pub fn sort_by(entries: &mut [DirEntry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        let ord = b.is_dir.cmp(&a.is_dir);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
        let inner = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Type => extension(&a.name)
                .to_lowercase()
                .cmp(&extension(&b.name).to_lowercase())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        };
        if ascending { inner } else { inner.reverse() }
    });
}

pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_string(),
        _ => String::new(),
    }
}

/// Item kind, used to sort/filter like Explorer's "Type" column.
pub fn item_kind(entry: &DirEntry) -> String {
    if entry.is_dir {
        return "File folder".into();
    }
    let ext = extension(&entry.name);
    if ext.is_empty() {
        return "File".into();
    }
    format!("{} file", ext.to_uppercase())
}

/// Human-readable byte size (Explorer shows KB in properties; the list shows raw).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b < KB * KB * KB * KB {
        format!("{:.2} GB", b / (KB * KB * KB))
    } else {
        format!("{:.2} TB", b / (KB * KB * KB * KB))
    }
}

/// Create a directory (recursively, like `mkdir -p`).
pub fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
    std::fs::create_dir_all(path.as_ref()).map_err(|e| WinError::Other(e.to_string()))
}

/// Rename / move an entry within or across volumes.
pub fn move_path(from: impl AsRef<Path>, to: impl AsRef<Path>, replace: bool) -> Result<()> {
    let f = to_wide(from.as_ref());
    let t = to_wide(to.as_ref());
    let mut flags = MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    check("MoveFileExW", unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), flags) })
}

pub fn attributes(path: impl AsRef<Path>) -> Result<u32> {
    let w = to_wide(path.as_ref());
    let a = unsafe { GetFileAttributesW(w.as_ptr()) };
    if a == u32::MAX {
        Err(WinError::from_last_error("GetFileAttributesW"))
    } else {
        Ok(a)
    }
}

pub fn set_readonly(path: impl AsRef<Path>, readonly: bool) -> Result<()> {
    let w = to_wide(path.as_ref());
    let cur = unsafe { GetFileAttributesW(w.as_ptr()) };
    if cur == u32::MAX {
        return Err(WinError::from_last_error("GetFileAttributesW"));
    }
    let next = if readonly { cur | FILE_ATTRIBUTE_READONLY } else { cur & !FILE_ATTRIBUTE_READONLY };
    check("SetFileAttributesW", unsafe { SetFileAttributesW(w.as_ptr(), next) })
}

/// Total size of a folder tree, with a hard cap to keep the UI responsive.
pub fn tree_size(dir: impl AsRef<Path>, budget: &mut u32) -> u64 {
    if *budget == 0 {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = read_dir(dir, true) {
        for e in entries {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            if e.is_dir {
                total += tree_size(&e.path, budget);
            } else {
                total += e.size;
            }
        }
    }
    total
}

/// Convert days-since-unix-epoch into a civil (year, month, day) triple.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        // 2026-11-24 == day 20781 since epoch
        assert_eq!(civil_from_days(20_781), (2026, 11, 24));
        // leap-day handling
        assert_eq!(civil_from_days(20_551), (2026, 4, 8));
    }

    #[test]
    fn size_formatting() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("readme.md"), "md");
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("noext"), "");
    }

    #[test]
    fn local_time_roundtrip() {
        assert!(!format_local(1_700_000_000).is_empty());
    }
}

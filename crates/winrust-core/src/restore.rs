//! System Restore integration and file-level backups.
//!
//! Two independent safety layers are offered to the user before any WinRust
//! component replaces a system binary:
//!
//! 1. **System Restore point** — `SRSetRestorePointW` with
//!    `MODIFY_SETTINGS`.  This is the same mechanism Control Panel → System
//!    Protection uses, so the snapshot can be rolled back from Windows itself
//!    even if WinRust is uninstalled.
//! 2. **File backup** — the exact binaries about to be replaced are copied into
//!    `%PROGRAMDATA%\WinRust\backup\<timestamp>\` together with a small JSON
//!    manifest, so a restore needs nothing but `copy`.
//!
//! Both are optional and independent: a machine may have System Restore
//! disabled by policy (common on corporate images), in which case the file
//! backup still protects the user.

use crate::error::{Result, WinError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Restore::{
    SRSetRestorePointW, MODIFY_SETTINGS, RESTOREPOINTINFOW, STATEMGRSTATUS,
};

/// Description limit enforced by the API (`MAX_DESC_W`); longer strings are
/// silently truncated by Windows, so we truncate explicitly and keep it valid.
const MAX_DESC_W: usize = 256;

/// Outcome of a restore-point request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePointResult {
    pub created: bool,
    pub sequence: i64,
    /// Human-readable reason when creation failed (policy disabled, no admin,
    /// System Protection off, ...).
    pub message: String,
}

/// Create a manual restore point ("Checkpoint") with the given description.
///
/// Requires an elevated process; on a non-elevated call Windows returns
/// `ERROR_ACCESS_DENIED` (5) and we surface that verbatim in `message` so the
/// UI can tell the user to relaunch as administrator.
pub fn create_restore_point(description: &str) -> RestorePointResult {
    let mut desc: Vec<u16> = description.encode_utf16().take(MAX_DESC_W - 1).collect();
    desc.push(0);
    let mut info: RESTOREPOINTINFOW = unsafe { std::mem::zeroed() };
    info.dwEventType = windows_sys::Win32::System::Restore::BEGIN_SYSTEM_CHANGE;
    info.dwRestorePtType = MODIFY_SETTINGS;
    info.llSequenceNumber = 0;
    // `RESTOREPOINTINFOW` is `#[repr(packed)]` (`DWORD`-aligned), so the array
    // -- like every field of every Win32 packed struct below -- has to be
    // written through raw pointers: creating a `&mut [u16]` into a misaligned
    // field would be undefined behaviour (rustc's E0793).
    let len = desc.len().min(MAX_DESC_W);
    unsafe {
        let dst = std::ptr::addr_of_mut!(info.szDescription).cast::<u16>();
        std::ptr::copy_nonoverlapping(desc.as_ptr(), dst, len);
    }

    let mut status: STATEMGRSTATUS = unsafe { std::mem::zeroed() };
    let ok = unsafe { SRSetRestorePointW(&info, &mut status) };
    // Read packed fields into locals before formatting them.
    let (seq, state) = unsafe {
        (
            std::ptr::addr_of!(status.llSequenceNumber).read_unaligned(),
            std::ptr::addr_of!(status.nStatus).read_unaligned(),
        )
    };
    if ok == 0 {
        let code = state;
        return RestorePointResult {
            created: false,
            sequence: seq,
            message: if code == 0 {
                "System Restore refused the request (System Protection may be disabled)".to_string()
            } else {
                format!(
                    "System Restore error {}: {}",
                    code,
                    crate::format_error(code)
                )
            },
        };
    }
    RestorePointResult {
        created: true,
        sequence: seq,
        message: format!("Restore point #{seq} created"),
    }
}

/// Whether System Restore is enabled for the system drive, as configured in
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore`.
///
/// This is informational only: the authoritative answer comes from the API
/// call itself, which is why [`create_restore_point`] is always attempted.
pub fn restore_enabled_hint() -> bool {
    let disabled = crate::registry::read_machine_u32(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore",
        "DisableSR",
    )
    .unwrap_or(0);
    disabled == 0
}

/// Is the current process elevated?  Restore points and system-binary replacement
/// both require it, so the UI uses this to gate the install buttons.
pub fn is_elevated() -> bool {
    crate::user::current_user().elevated
}

// ---------------------------------------------------------------------------
// File backups
// ---------------------------------------------------------------------------

/// One file captured in a backup session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// Where the file was at backup time (absolute path).
    pub original: PathBuf,
    /// File name inside the backup directory.
    pub stored_as: String,
    pub existed: bool,
    pub size: u64,
}

/// A completed backup session, persisted next to the copied files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSession {
    pub created_unix: u64,
    pub label: String,
    pub restore_sequence: Option<i64>,
    pub entries: Vec<BackupEntry>,
}

impl BackupSession {
    pub fn directory(&self) -> PathBuf {
        backup_root().join(format!("{}", self.created_unix))
    }
}

/// Root of all WinRust backups: `%PROGRAMDATA%\WinRust\backup`.
pub fn backup_root() -> PathBuf {
    let base = std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("WinRust").join("backup")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Copy every `path` that exists into a fresh timestamped backup directory and
/// write `session.json`.  Missing files are recorded with `existed: false` so a
/// rollback knows to delete a file that WinRust created from scratch.
pub fn backup_files(paths: &[PathBuf], label: &str, restore_sequence: Option<i64>) -> Result<BackupSession> {
    let created = now_unix();
    let dir = backup_root().join(created.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| WinError::Other(format!("create backup dir: {e}")))?;

    let mut entries = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let exists = path.exists();
        let mut size = 0u64;
        if exists {
            size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            // Keep the original file name but prefix with the index so two
            // files with the same name from different directories cannot clash.
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("file{index}"));
            let stored = format!("{index:02}_{name}");
            std::fs::copy(path, dir.join(&stored))
                .map_err(|e| WinError::Other(format!("backup {}: {e}", path.display())))?;
            entries.push(BackupEntry {
                original: path.clone(),
                stored_as: stored,
                existed: true,
                size,
            });
        } else {
            entries.push(BackupEntry {
                original: path.clone(),
                stored_as: String::new(),
                existed: false,
                size,
            });
        }
    }

    let session = BackupSession {
        created_unix: created,
        label: label.to_string(),
        restore_sequence,
        entries,
    };
    let json = serde_json::to_string_pretty(&session).map_err(|e| WinError::Other(e.to_string()))?;
    std::fs::write(dir.join("session.json"), json)
        .map_err(|e| WinError::Other(format!("write manifest: {e}")))?;
    Ok(session)
}

/// Restore a session: every captured file is copied back, and every file that
/// did not exist before is deleted again.
pub fn restore_session(session: &BackupSession) -> Result<Vec<String>> {
    let dir = session.directory();
    let mut log = Vec::new();
    for entry in &session.entries {
        if entry.existed {
            let src = dir.join(&entry.stored_as);
            match std::fs::copy(&src, &entry.original) {
                Ok(_) => log.push(format!("restored {}", entry.original.display())),
                Err(e) => log.push(format!("FAILED {}: {e}", entry.original.display())),
            }
        } else if entry.original.exists() {
            match std::fs::remove_file(&entry.original) {
                Ok(()) => log.push(format!("removed {}", entry.original.display())),
                Err(e) => log.push(format!("FAILED to remove {}: {e}", entry.original.display())),
            }
        }
    }
    Ok(log)
}

/// All backup sessions on disk, newest first.
pub fn list_sessions() -> Vec<BackupSession> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(backup_root()) else {
        return out;
    };
    for entry in read.flatten() {
        let manifest = entry.path().join("session.json");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            if let Ok(session) = serde_json::from_str::<BackupSession>(&text) {
                out.push(session);
            }
        }
    }
    out.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));
    out
}

/// Delete every backup session older than `keep_newest` sessions.
pub fn prune_sessions(keep_newest: usize) -> usize {
    let sessions = list_sessions();
    let mut removed = 0;
    for session in sessions.into_iter().skip(keep_newest) {
        let dir = session.directory();
        if std::fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Does this path currently hold one of our own binaries?
pub fn is_winrust_binary(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().to_lowercase().starts_with("winrust"))
        .unwrap_or(false)
}

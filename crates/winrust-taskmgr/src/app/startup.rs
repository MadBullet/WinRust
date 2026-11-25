//! The Startup tab: what Windows would launch at sign-in, and switching an
//! entry on or off.
//!
//! Two places hold autostarts on Windows: the `Run` keys, and the per-user
//! `StartupApproved` keys that record whether the user disabled an entry.  The
//! list reads both, so a disabled entry still shows (greyed) instead of
//! vanishing.

use winrust_core::registry::{self, Root};
use winrust_core::shell;

use super::model::exe_from_command;

#[derive(Debug, Clone)]
pub(super) struct StartupItem {
    pub(super) name: String,
    pub(super) command: String,
    pub(super) location: String,
    pub(super) enabled: bool,
    pub(super) published: String,
}

impl StartupItem {
    pub(super) fn scan() -> Vec<StartupItem> {
        let mut out = Vec::new();
        for (root, path, label) in [
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                "HKCU Run",
            ),
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                "HKLM Run",
            ),
        ] {
            for (name, ty, data) in registry::enum_values(path, root) {
                let cmd = if ty == 1 || ty == 2 {
                    String::from_utf16_lossy(
                        &data
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect::<Vec<u16>>(),
                    )
                    .trim_end_matches('\0')
                    .to_string()
                } else {
                    String::new()
                };
                let exe = exe_from_command(&cmd);
                let published = if exe.is_empty() {
                    String::new()
                } else {
                    let vi = shell::version_info(&exe);
                    if vi.company.is_empty() {
                        vi.description
                    } else {
                        vi.company
                    }
                };
                out.push(StartupItem {
                    name,
                    command: cmd,
                    location: label.into(),
                    enabled: true,
                    published,
                });
            }
        }
        // StartupApproved\Run holds a 12-byte blob: first dword 2 = enabled,
        // 3 = disabled.  This is what Task Manager toggles.
        for (root, approved_path) in [
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
        ] {
            for (name, _ty, data) in registry::enum_values(approved_path, root) {
                if data.len() < 4 {
                    continue;
                }
                let flag = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let disabled = flag & 0xFF != 2;
                if let Some(item) = out.iter_mut().find(|i| i.name.eq_ignore_ascii_case(&name)) {
                    item.enabled = !disabled;
                }
            }
        }
        out.sort_by_key(|s| s.name.to_lowercase());
        out
    }

    pub(super) fn set_enabled(item: &StartupItem, enable: bool) -> Result<(), String> {
        use winrust_core::registry::{self, Root};
        let (root, path) = if item.location.starts_with("HKLM") {
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            )
        } else {
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            )
        };
        let key = registry::open_for_write(root, path).map_err(|e| e.to_string())?;
        let mut blob = [0u8; 12];
        blob[0] = if enable { 2 } else { 3 };
        key.set_binary(&item.name, &blob).map_err(|e| e.to_string())
    }
}

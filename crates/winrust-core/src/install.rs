//! Selective replacement of Windows binaries with their WinRust equivalents.
//!
//! Three integration levels are implemented, each opt-in per component and
//! each fully reversible:
//!
//! | Level | Mechanism | Scope |
//! |-------|-----------|-------|
//! | **Portable** | just run the .exe from its own folder | no system files touched |
//! | **Hijack** | `IFEO\Image File Execution Options\<exe>` `Debugger` value | per-user, per-app |
//! | **Replace** | backup + overwrite the system binary | machine-wide, needs admin |
//!
//! Every change is recorded in the [`crate::restore`] backup so [`uninstall`]
//! can undo it exactly, and the caller can additionally create a System Restore
//! point first.

use crate::error::{Result, WinError};
use crate::registry::{self, Root};
use crate::wide::to_wide;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING,
};

/// How deeply a component integrates with the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Integration {
    /// Nothing installed; the component is only launched on demand.
    Portable,
    /// `IFEO\...\Debugger` redirect — Windows' own "debugger" hook, which is
    /// how tools such as Process Explorer replace Task Manager.  Reversible,
    /// per-machine but only affects the one executable name.
    Hijack,
    /// Binary replaced in `%SystemRoot%\System32` (or the component's own
    /// folder).  Highest fidelity, requires elevation, must be backed up.
    /// Note: Windows Resource Protection may still refuse to overwrite some
    /// system files; in that case WinRust schedules the swap for the next
    /// reboot via `PendingFileRenameOperations`.
    Replace,
    /// `Winlogon\Shell` override — the documented way to run a custom shell.
    /// This is the reliable route for Explorer, where IFEO is ignored on
    /// Windows 10/11.
    Shell,
}

impl Integration {
    pub fn label(self) -> &'static str {
        match self {
            Integration::Portable => "Portable (no changes)",
            Integration::Hijack => "Redirect (IFEO)",
            Integration::Replace => "Replace system binary",
            Integration::Shell => "Custom shell (Winlogon)",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Integration::Portable => "Runs from the WinRust folder. Nothing in Windows changes.",
            Integration::Hijack => "Windows starts the WinRust build whenever the original exe is launched. Undo is one registry delete.",
            Integration::Replace => "The original file is backed up, then overwritten. Requires administrator rights.",
            Integration::Shell => "Windows starts this program as the desktop shell at sign-in. Undo removes the override.",
        }
    }
    pub fn requires_admin(self) -> bool {
        matches!(
            self,
            Integration::Hijack | Integration::Replace | Integration::Shell
        )
    }

    /// Only a desktop shell can be a sign-in shell.
    pub fn applies_to_shell(self) -> bool {
        matches!(self, Integration::Shell)
    }
}

/// Which system binary a component stands in for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSpec {
    /// Executable name the component replaces (`explorer.exe`).
    pub exe: String,
    /// Absolute path that would be replaced, when known.
    pub system_path: Option<PathBuf>,
    /// Registry path for the IFEO redirect.
    pub ifeo_path: String,
}

impl TargetSpec {
    pub fn for_exe(exe: &str) -> Self {
        // IFEO lives under HKLM so the redirect applies to every user.
        let ifeo_path = format!(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\{}",
            exe
        );
        Self {
            exe: exe.to_string(),
            system_path: locate_system_binary(exe),
            ifeo_path,
        }
    }
}

/// Find the real location of a system executable.
///
/// `explorer.exe` lives in `%SystemRoot%`, most others in `System32`.
pub fn locate_system_binary(exe: &str) -> Option<PathBuf> {
    let windir = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let candidates = [
        windir.join(exe),
        windir.join("System32").join(exe),
        windir.join("SysWOW64").join(exe),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Result of an install step, shown verbatim in the launcher UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReport {
    pub component: String,
    pub integration: Integration,
    pub actions: Vec<String>,
    pub success: bool,
    pub error: Option<String>,
}

/// Install one component.
///
/// `source` is the WinRust binary; `spec` describes what it replaces.
/// On success the caller should record the returned actions in the log.
pub fn install(
    component: &str,
    source: &Path,
    spec: &TargetSpec,
    integration: Integration,
) -> InstallReport {
    let mut actions = Vec::new();
    let result = (|| -> Result<()> {
        if !source.is_file() {
            return Err(WinError::Other(format!(
                "source binary missing: {}",
                source.display()
            )));
        }
        match integration {
            Integration::Portable => {
                actions.push(format!("Portable: {}", source.display()));
                Ok(())
            }
            Integration::Hijack => {
                let key = registry::open_for_write(Root::LocalMachine, &spec.ifeo_path)?;
                key.set_string("Debugger", &format!("\"{}\"", source.display()))?;
                actions.push(format!(
                    "Set HKLM\\{} Debugger = \"{}\"",
                    spec.ifeo_path,
                    source.display()
                ));
                Ok(())
            }
            Integration::Shell => {
                // `source` is already the staged copy in %ProgramFiles%\WinRust,
                // which is a stable location that survives the launcher folder.
                set_shell(Some(source))?;
                actions.push(format!(
                    "Winlogon Shell = \"{}\" (sign-in starts WinRust)",
                    source.display()
                ));
                actions
                    .push("explorer.exe is untouched — remove the override to get it back".into());
                Ok(())
            }
            Integration::Replace => {
                let target = spec.system_path.clone().ok_or_else(|| {
                    WinError::Other(format!("{} not found on this system", spec.exe))
                })?;
                // The backup itself happens in `restore::backup_files` before
                // this call; here we only swap the file.
                match std::fs::copy(source, &target) {
                    Ok(_) => {
                        actions.push(format!(
                            "Replaced {} with {}",
                            target.display(),
                            source.display()
                        ));
                    }
                    Err(_) => {
                        // A running shell binary cannot be overwritten in place
                        // (sharing violation).  Windows' own updaters schedule the
                        // swap for the next boot through the PendingFileRename
                        // registry list, which `MOVEFILE_DELAY_UNTIL_REBOOT` writes.
                        let from = to_wide(source);
                        let to = to_wide(&target);
                        let ok = unsafe {
                            MoveFileExW(
                                from.as_ptr(),
                                to.as_ptr(),
                                MOVEFILE_REPLACE_EXISTING | MOVEFILE_DELAY_UNTIL_REBOOT,
                            )
                        };
                        if ok == 0 {
                            return Err(WinError::Other(format!(
                                "replace {}: file is in use and the reboot swap was refused ({})",
                                target.display(),
                                crate::format_error(
                                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                                        as u32
                                )
                            )));
                        }
                        actions.push(format!(
                            "Scheduled replacement of {} at next reboot (file was in use)",
                            target.display()
                        ));
                    }
                }
                Ok(())
            }
        }
    })();

    match result {
        Ok(()) => InstallReport {
            component: component.to_string(),
            integration,
            actions,
            success: true,
            error: None,
        },
        Err(e) => InstallReport {
            component: component.to_string(),
            integration,
            actions,
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Undo an installation: remove the IFEO redirect, or restore the backed-up
/// system binary when the backup still has it.
pub fn uninstall(
    component: &str,
    spec: &TargetSpec,
    integration: Integration,
    backup: Option<&crate::restore::BackupSession>,
) -> InstallReport {
    let mut actions = Vec::new();
    let result = (|| -> Result<()> {
        match integration {
            Integration::Portable => {
                actions.push("Portable: nothing to undo".into());
                Ok(())
            }
            Integration::Hijack => {
                if !hijack_active(spec) {
                    actions.push("IFEO redirect was not present".into());
                } else {
                    registry::delete_value(Root::LocalMachine, &spec.ifeo_path, "Debugger")?;
                    actions.push(format!("Removed IFEO redirect for {}", spec.exe));
                }
                Ok(())
            }
            Integration::Shell => {
                let current = configured_shell();
                set_shell(None)?;
                actions.push(match current {
                    Some(c) => format!("Removed Winlogon Shell override (was {c}); sign-in returns to explorer.exe"),
                    None => "Winlogon Shell override was not set".into(),
                });
                Ok(())
            }
            Integration::Replace => {
                let Some(session) = backup else {
                    return Err(WinError::Other(
                        "no backup recorded for this component — restore it from System Restore"
                            .into(),
                    ));
                };
                let log = crate::restore::restore_session(session)?;
                actions.extend(log);
                Ok(())
            }
        }
    })();

    match result {
        Ok(()) => InstallReport {
            component: component.to_string(),
            integration,
            actions,
            success: true,
            error: None,
        },
        Err(e) => InstallReport {
            component: component.to_string(),
            integration,
            actions,
            success: false,
            error: Some(e.to_string()),
        },
    }
}

/// Is a component currently redirected through IFEO?
pub fn hijack_active(spec: &TargetSpec) -> bool {
    registry::Key::open(Root::LocalMachine, &spec.ifeo_path)
        .ok()
        .and_then(|k| k.string("Debugger"))
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Is this system binary already one of ours (i.e. was replaced before)?
pub fn replacement_active(spec: &TargetSpec) -> bool {
    spec.system_path
        .as_ref()
        .map(|p| crate::restore::is_winrust_binary(p))
        .unwrap_or(false)
}

/// Copy our binary next to the target under a `<name>.winrust` name, so the
/// original file name stays free for the swap.  Used by [`Integration::Replace`].
pub fn side_load(source: &Path, target: &Path) -> Result<PathBuf> {
    let dir = target
        .parent()
        .ok_or_else(|| WinError::Other("target has no directory".into()))?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "target.exe".into());
    let staged = dir.join(format!("{name}.winrust"));
    std::fs::copy(source, &staged)
        .map_err(|e| WinError::Other(format!("side-load {}: {e}", staged.display())))?;
    Ok(staged)
}

/// Registry path where the launcher remembers what it installed.
///
/// `HKLM\SOFTWARE\WinRust\Installed` holds one `REG_SZ` per component:
/// `explorer` = `hijack`, `taskmgr` = `replace`, plus `explorer.target` =
/// the binary that was overwritten.  Surviving a launcher restart is what makes
/// "Restore" work days later.
/// Machine-wide Winlogon key holding the shell command.
pub const WINLOGON_PATH: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";

/// The shell currently configured for sign-in, if any override exists.
pub fn configured_shell() -> Option<String> {
    registry::Key::open(Root::LocalMachine, WINLOGON_PATH)
        .ok()
        .and_then(|k| k.string("Shell"))
        .filter(|s| !s.is_empty())
}

/// Override the sign-in shell.  `None` removes the value, which restores the
/// Windows default (`explorer.exe`).
pub fn set_shell(shell: Option<&Path>) -> Result<()> {
    match shell {
        Some(path) => {
            let key = registry::open_for_write(Root::LocalMachine, WINLOGON_PATH)?;
            key.set_string("Shell", &format!("\"{}\"", path.display()))
        }
        None => {
            let _ = registry::delete_value(Root::LocalMachine, WINLOGON_PATH, "Shell");
            Ok(())
        }
    }
}

pub const STATE_PATH: &str = r"SOFTWARE\WinRust\Installed";

fn state_token(level: Integration) -> &'static str {
    match level {
        Integration::Portable => "portable",
        Integration::Hijack => "hijack",
        Integration::Replace => "replace",
        Integration::Shell => "shell",
    }
}

fn parse_token(value: &str) -> Integration {
    match value {
        "hijack" => Integration::Hijack,
        "replace" => Integration::Replace,
        "shell" => Integration::Shell,
        _ => Integration::Portable,
    }
}

/// Persist the integration level applied to `id`.
pub fn record_state(id: &str, level: Integration, target: Option<&Path>) -> Result<()> {
    let key = registry::open_for_write(Root::LocalMachine, STATE_PATH)?;
    key.set_string(id, state_token(level))?;
    if let Some(t) = target {
        key.set_string(&format!("{id}.target"), &t.display().to_string())?;
    }
    Ok(())
}

/// Read back the integration level recorded for `id`.
pub fn read_state(id: &str) -> Option<Integration> {
    registry::Key::open(Root::LocalMachine, STATE_PATH)
        .ok()
        .and_then(|k| k.string(id))
        .map(|v| parse_token(&v))
}

/// The binary recorded as replaced for `id`.
pub fn recorded_target(id: &str) -> Option<PathBuf> {
    registry::Key::open(Root::LocalMachine, STATE_PATH)
        .ok()
        .and_then(|k| k.string(&format!("{id}.target")))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Forget everything recorded for `id` (after a successful restore).
pub fn clear_state(id: &str) -> Result<()> {
    for name in [id.to_string(), format!("{id}.target")] {
        let _ = registry::delete_value(Root::LocalMachine, STATE_PATH, &name);
    }
    Ok(())
}

/// Where WinRust keeps its own binaries when installed machine-wide.
///
/// `%ProgramFiles%\WinRust` is used so the replaced system binaries can be
/// restored without depending on where the user unzipped the release.
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join("WinRust")
}

/// Copy the whole component set into [`install_dir`] and return the copied
/// paths keyed by component id.
pub fn stage_binaries(sources: &[(String, PathBuf)]) -> Result<Vec<(String, PathBuf)>> {
    let dir = install_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| WinError::Other(format!("create {}: {e}", dir.display())))?;
    let mut out = Vec::with_capacity(sources.len());
    for (id, src) in sources {
        let dst = dir.join(
            src.file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("winrust.exe")),
        );
        std::fs::copy(src, &dst)
            .map_err(|e| WinError::Other(format!("stage {}: {e}", src.display())))?;
        out.push((id.clone(), dst));
    }
    Ok(out)
}

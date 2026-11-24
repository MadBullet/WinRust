//! Installing and starting the driver as a kernel service.
//!
//! A KMDF driver is a service of type `SERVICE_KERNEL_DRIVER` pointing at the
//! `.sys` image; `sc create` does the same thing through the SCM API we already
//! wrap in `winrust-core::service`.  Using the wrapper keeps behaviour identical
//! to the Services app, and gives us the real Win32 error text on failure.

use std::path::Path;
use winrust_core::error::{Result, WinError};
use winrust_core::registry::{self, Root};
use winrust_core::service::ScManager;

/// Service name used by the INF and by the control tool.
pub const SERVICE_NAME: &str = "WinRustKernel";
/// Where the staged `.sys` lives.
pub const DRIVER_FILE: &str = "WinRustKernel.sys";

/// Where WinRust stages the driver image: `%ProgramFiles%\WinRust\driver`.
pub fn driver_dir() -> std::path::PathBuf {
    let base = std::env::var_os("ProgramFiles")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files"));
    base.join("WinRust").join("driver")
}

/// Copy `source` into [`driver_dir`] under the canonical file name.
pub fn stage(source: &Path) -> Result<std::path::PathBuf> {
    let dir = driver_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| WinError::Other(format!("create {}: {e}", dir.display())))?;
    let target = dir.join(DRIVER_FILE);
    std::fs::copy(source, &target)
        .map_err(|e| WinError::Other(format!("copy {}: {e}", source.display())))?;
    Ok(target)
}

/// Is the service registered?
pub fn is_installed() -> bool {
    ScManager::connect()
        .ok()
        .and_then(|scm| scm.enumerate().ok())
        .map(|list| {
            list.iter()
                .any(|s| s.name.eq_ignore_ascii_case(SERVICE_NAME))
        })
        .unwrap_or(false)
}

/// Register the service pointing at `image`.
pub fn install(image: &Path) -> Result<()> {
    let scm = ScManager::connect()?;
    scm.create_kernel_service(SERVICE_NAME, "WinRust kernel driver", image)
}

/// Start the service (loads the driver).
pub fn start() -> Result<()> {
    ScManager::connect().and_then(|scm| scm.open_service(SERVICE_NAME, SERVICE_START)?.start())
}

/// Stop the service (unloads the driver).
pub fn stop() -> Result<()> {
    ScManager::connect().and_then(|scm| scm.open_service(SERVICE_NAME, SERVICE_STOP)?.stop())
}

/// Access mask for `StartServiceW`.
const SERVICE_START: u32 = 0x0010;
/// Access mask for `ControlService(STOP)`.
const SERVICE_STOP: u32 = 0x0020;

/// Delete the service and remove our copy of the image.
pub fn remove() -> Result<()> {
    if is_installed() {
        let scm = ScManager::connect()?;
        scm.delete(SERVICE_NAME)?;
    }
    let image = driver_dir().join(DRIVER_FILE);
    if image.is_file() && std::fs::remove_file(&image).is_err() {
        // The image is still mapped by a running driver, or a pending reboot
        // swap owns it: ask Windows to delete it at next boot.
        schedule_delete(&image);
    }
    // The driver has no persistent state, but clean the record anyway.
    let _ = registry::delete_key(Root::LocalMachine, r"SOFTWARE\WinRust", "Kernel");
    Ok(())
}

/// Queue `path` for deletion at next boot via `MOVEFILE_DELAY_UNTIL_REBOOT`.
fn schedule_delete(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
    let wide = winrust_core::wide::to_wide(path);
    unsafe {
        MoveFileExW(wide.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
    }
}

/// Reboot is needed when the image is in use and Windows deferred the swap.
pub fn reboot_pending() -> bool {
    registry::Key::open(
        Root::LocalMachine,
        r"SYSTEM\CurrentControlSet\Control\Session Manager",
    )
    .ok()
    .and_then(|key| key.value_raw(Some("PendingFileRenameOperations")).ok())
    .map(|(_, data)| {
        let text = String::from_utf8_lossy(&data).to_lowercase();
        text.contains("winrust")
    })
    .unwrap_or(false)
}

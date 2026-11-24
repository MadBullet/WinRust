Windows 10/11 system components rewritten in Rust: Explorer, Task Manager, Services,
system utilities, a shared Win32 core library and - new in 0.4.0 - a real kernel driver.

## Kernel driver (new)

* `WinRustKernel.sys` - KMDF driver built from Rust with the WDK. Kernel-side process
  and module tables (it sees PPL-protected processes that `OpenProcess` refuses) and an
  in-RAM process-creation block list.
* `winrust-driver.exe` - console: `install`, `status`, `processes`, `modules`, `perf`,
  `block <name>`, `allow <name>`, `remove`.
* Install from an elevated prompt: `winrust-driver install WinRustKernel.sys`, then
  `winrust-driver processes`. `winrust-driver remove` unloads and unregisters it.
* The image is built but **not** production-signed. To load it on your own machine
  enable test signing (`bcdedit /set testsigning on`, reboot) or sign it with your own
  certificate. This is a real kernel driver: try it in a VM first.

## Launcher `WinRust.exe` is a selective installer

* One checkbox per component - install only what you want.
* Per component: Portable / IFEO redirect / replace system binary / custom shell.
* System Restore point (`SRSetRestorePointW`) and a file-level backup in
  `%ProgramData%\WinRust\backup`, both on by default.
* Backup browser with one-click rollback; installed state persisted in
  `HKLM\SOFTWARE\WinRust\Installed`.
* Everything is written to the activity log in the window.

## Downloads

* `WinRust-windows-x64.zip` - every executable, the driver and the README.
* Or grab a single file below.

Elevation (Run as administrator) is required for service control, for managing other
users' processes, and for any install action.

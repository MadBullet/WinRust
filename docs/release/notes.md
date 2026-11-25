Windows 10/11 system components rewritten in Rust. Release 0.5.0 adds a complete
desktop shell, a Russian interface for every program, and fixes to the Task Manager
crash and to transparency.

## WinRust Desktop (new)

`winrust-desktop.exe` is a standalone shell that replaces `explorer.exe` at sign-in:

* Desktop icons from your real Desktop folder, on a snapping grid. Positions are kept
  in `%APPDATA%\WinRust\desktop.json`, so the shell does not reshuffle your desktop.
* Taskbar: Start, pinned WinRust tools, every open window (click to bring it forward),
  live CPU/RAM, clock, and a status line for messages.
* Start menu: search across both Start Menu folders, known folders, running windows,
  power actions, and **Back to Windows shell**.
* Installed through `Winlogon\Shell` - no system file is replaced. Undo removes the
  value and starts `explorer.exe` again.

## Russian interface, English by default

* Every component (launcher, desktop, Explorer, Task Manager, Services, system
  utilities) is fully translated - 418 strings, no duplicates, matching placeholders.
* The language switcher lives in the launcher header. The choice is stored in
  `HKCU\SOFTWARE\WinRust` (no elevation) and already-open windows follow within a
  second, so switching language no longer requires restarting anything.
* Untranslated strings fall back to English rather than showing a placeholder.

## Transparency and blur

* The launcher's Settings page sets transparency, opacity and acrylic blur.
* Off by default: an opaque window costs nothing extra. Transparency uses the layered
  window alpha, blur uses `DwmEnableBlurBehindWindow`; on systems without it the dark
  theme simply stays opaque instead of failing.
* Fixed in this release: the opacity slider had no effect, because the theme ignored
  its settings argument.

## Task Manager

* **The crash is fixed.** The release profile uses `panic = "abort"`, and the old code
  sliced the executable path by bytes - which panics on the Cyrillic profile paths
  (`C:\Users\Иван\...`) that a Russian Windows always has. All truncation now goes
  through character-safe helpers, the sampler is guarded by `catch_unwind`, and the
  header and column count can no longer disagree.
* **GPU column per process**, from PDH `GPU Engine` counters: utilisation plus the
  busiest engine name, with the same heat colouring as CPU, memory, disk and network.

## Explorer

* **This PC** now has the real Windows layout: folder tiles on top, then devices and
  drives with horizontal capacity bars and "N GB free of M GB".
* Rename no longer swallows the rest of the view when it runs.

## Services

* Click any column header to sort - name, description, status, start type, logon
  account - and click again to reverse. Same as `services.msc`.

## Launcher

* Ships `winrust-desktop.exe` in the zip and as a release asset.
* Install / uninstall reporting, the safety-net panel, shell takeover and the driver
  controls are all translated and borrow their strings instead of moving them.

## Kernel driver

* `WinRustKernel.sys` - KMDF driver built from Rust with the WDK. Kernel-side process
  and module tables (it sees PPL-protected processes that `OpenProcess` refuses) and an
  in-RAM process-creation block list.
* `winrust-driver.exe` - console: `install`, `status`, `processes`, `modules`, `perf`,
  `block <name>`, `allow <name>`, `remove`.
* The image is built but not production-signed. To load it on your own machine
  enable test signing (`bcdedit /set testsigning on`, reboot) or sign it with your own
  certificate. This is a real kernel driver: try it in a VM first.

## Launcher `WinRust.exe` is a selective installer

* One checkbox per component - install only what you want.
* Per component: Portable / IFEO redirect / replace system binary / shell.
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

Windows 10/11 system components rewritten in Rust. Release 0.6.0 is the
appearance-and-shell release: three themes shared by every window, a blur effect
that finally works, a desktop that behaves like Windows 11, and a Task Manager
with a real Details tab.

## Themes: Dark, Light and AMOLED

* Every program reads the same three palettes from
  `HKCU\SOFTWARE\WinRust\Theme`: **Dark** (the classic WinRust scheme), **Light**
  (Windows 11 grey) and **AMOLED** (the dark layout on pure black, for OLED
  panels). The switch lives on the launcher's Appearance page.
* Fixed: with transparency off, windows could come out light/white until the
  switch was toggled twice. `Context::set_visuals` only wrote the style bucket of
  the *active* theme, so a dark palette landed in the light bucket on a machine
  whose Windows preference is Light. The theme preference is now pinned and both
  buckets are written.
* Windows 11 wins: rounded corners and the dark title bar follow the theme.

## Blur works

* Transparency and blur are now separate, exclusive DWM paths. Blur removes
  `WS_EX_LAYERED` and asks DWM for an acrylic backdrop
  (`DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`), falling back to the
  classic `DwmExtendFrameIntoClientArea` + `DwmEnableBlurBehindWindow` blur on
  older builds. Blur off sets the backdrop to none and applies the opacity
  slider through the layered window, which is why the two effects used to cancel
  each other out.

## Explorer

* **Double-click opens a drive** in This PC. The tiles only reacted to single
  clicks, so a double click re-opened the same folder and looked dead.
* **Drag and drop** works between tiles, the details/list views and the sidebar
  tree. The drag image is the item's real shell icon, drawn translucent under
  the cursor, with a name card (mid-truncated, extension kept) and a count badge.
* **Icon spacing** is configurable from the background context menu and saved per
  folder, together with the tiles size (large / medium / small).
* The background **context menu** is Explorer's: View, Sort by (with direction),
  Refresh, Paste, New (folder, text document, bitmap, ZIP), Select all, Invert
  selection, Send to desktop, Properties.

## Task Manager

* New **Details** tab: one line per process with name, PID, status, user name,
  CPU, memory, disk, network, GPU, threads, handles, session, priority and the
  full image path - and every one of those columns sorts on click.
* Sorted every column of the Processes tab on its own value; Status and Priority
  no longer reuse the PID key.
* Processes are grouped under their parent application, with the `↳` marker for
  helpers, and both group captions (**Apps (N)** / **Background processes (N)**)
  collapse with their aggregate CPU and memory.

## Desktop shell

* Desktop icons use the **real icon of the shortcut target**, loaded from the
  shell image list at the chosen size.
* Wallpaper context menu: View (large/medium/small icons, align to grid, show
  icons), Sort by (name, size, type, date modified, direction), Refresh, New
  folder, Display settings, Personalize.
* Dragging an icon paints a translucent ghost of its picture plus a short label
  and outlines the target cell; positions are persisted.
* The taskbar is centred like Windows 11: Start, pinned tools, one button per
  open window with the running underline, and a tray with chevron, clock/date,
  live CPU/RAM and the power button.
* The Start menu follows the reference layout: **ПУСК** caption, a tile grid,
  the account row (change account settings, lock, sign out, switch user),
  Settings and a power menu (sleep, shut down, restart, lock).

## Everything else

* `ProcessInfo` now carries the owning account (resolved through the process
  token, cached by SID) and the session id.
* `disk::copy_path` for clipboard-style copies.
* 455 localized strings, no duplicate keys.

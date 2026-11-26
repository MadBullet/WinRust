Windows 10/11 system components rewritten in Rust. Release 0.9.1 is the
correctness release for what 0.9.0 put on screen: four groups of bugs found in
review, each fixed at its cause, and every structural change run through
[deslop-please](https://github.com/deterministic-machines/deslop-please) again.

## 1. The launcher now knows what it is allowed to do

The review's first finding was blunt: start the launcher as an administrator
and it still calls you a standard user, then refuses to install. Three things
were wrong at once:

* one boolean (`elevated`) answered three different questions - *is this
  process elevated*, *is the account an administrator*, and *could it elevate
  by relaunching* - so a UAC-limited administrator was indistinguishable from
  an ordinary user;
* the token query returned `false` on any failure, hiding the real error behind
  a policy-looking answer;
* the only advice the UI had was "relaunch as administrator", with no way to do
  it.

`Privilege` (new, `winrust-core::system::privilege`) is now the single answer:
`elevated`, `administrator`, `can_elevate`, plus `may_modify_system()` and a
label. It asks Windows with `TokenElevation`, `TokenElevationType` **and**
`CheckTokenMembership` against the Administrators SID, so "limited admin" is
reported as what it is. `UserInfo` embeds it, so no call site moved.

The launcher shows the right badge, and when the answer is "you could elevate"
it offers a **Restart as administrator** button that goes through
`ShellExecuteExW("runas")` - the documented consent path - instead of an error
message telling the user to go find the exe.

## 2. The Start menu and the taskbar

* **It closes as fast as it opens.** One easing rate (14/s) served both
  directions, so dismissal took ~7 frames. Opening is 22/s and closing 45/s,
  the same asymmetry Windows uses.
* **It sits where it belongs.** The panel is anchored to the taskbar's top
  edge, and its height is capped by the space actually left on screen; the
  footer can no longer be pushed behind the bar.
* **Two columns, not sixty rectangles.** Left: every installed app once,
  alphabetical, with the icon the shell reports. Right: pinned apps as square
  icon tiles with the name underneath. Searching filters the left column.
* **One Settings, one account, one power button.** The power button opens a
  list - shut down, restart, sleep, hibernation, sign out - and **every entry
  asks for confirmation** in its own dialog. `Win+X` uses the same path, so a
  misclick cannot shut the machine down. Hibernation is greyed out when
  `powercfg /hibernate` is off, instead of silently doing nothing.
  Sleep and hibernation now call `SetSuspendState` directly; routing
  `SetSuspendState` through `rundll32` hibernates on many systems even when
  "sleep" was requested.
* **Tray flyouts hang from their button.** They used to be placed by
  subtracting a *guessed* height from the bar, so the calendar floated up to
  60 px above the bar. A flyout is now anchored by its top edge under the
  button that opened it and grows downwards into the screen.
* **Running and active are two different signals.** Running: a short blue line
  (16 px, wider per extra window) on the bar's inner edge. Active: a soft
  translucent fill behind the button. Hover is a third, weaker fill, so it can
  no longer mask "active".
* **The preview is a thumbnail at the mouse.** It is centred on the hovered
  button instead of pinned to the tray's right edge, its card is 84-178 px wide
  and 80-150 px tall chosen from the window's aspect ratio, and windows that do
  not really exist are filtered out - it is thumbnail-first, title second.
* **`Win+V` works again.** Windows' clipboard history lives in `explorer.exe`;
  with WinRust as the shell it stopped existing. The core now has a
  `shell::clipboard::History` (40 entries, de-duplicated, most recent first) and
  the desktop polls the system clipboard, opens the panel on `Win+V`, and puts a
  picked entry back on the clipboard.

## 3. Explorer

* **Double-clicking a drive opens the drive.** `is_computer_view()` also
  answered "yes" for a bare root like `C:\`, so navigating to a drive
  re-rendered the "This PC" tile grid and the click looked dead. A drive root
  is a folder now.
* **The navigation pane has a place to right-click.** A strip above the folder
  list carries the same background menu as the file area (refresh, paste, new
  folder).

## 4. Task Manager

* **Grouping no longer destroys the column sort.** With "group by app" on, the
  list was sorted by `(is_app, group)` only, so clicking a column appeared to do
  nothing. Grouping now partitions (apps, then background) and orders by group,
  using the chosen column as the tie-breaker - one `compare_rows` defines
  "what CPU means" for both the flat and the grouped view.
* **The app-child arrow means something.** The indent test compared the display
  name with the lowercased group key, which put an arrow on nearly every row.
* **The performance graph does not overlap.** The fill was a `convex_polygon`
  over a polyline that goes up and down - not a convex shape - so egui
  triangulated it as a fan and the shading spilled out of the area. It is a
  `Mesh` of one quad per interval now. The four graphs also shared a
  two-column layout whose columns were up to 460 px each and collided on a
  normal window; the page is one column that scrolls.

## What the linter reports

`field-leak` and `shallow-module` again, on the new seams:

| was | now |
| --- | --- |
| five loose Start fields on `DesktopApp` (`start_open`, `start_search`, `start_power_menu`, `start_anim`, app list) | `session::Session` with verbs (`open`, `close`, `toggle`, `ask_power`, `confirm_power`) |
| `hover_pid` + `hover_since` on `DesktopApp`, preview anchored to the screen | `taskbar::Hover` (pid, since, anchor) with `enter` / `clear` / `rested` |
| flyout placement guessed from a height table | `TrayState` hands out `toggle_from(button, bar_top)` and `anchor()` |
| "is this an administrator?" asked in four places | `Privilege::current()` once, embedded in `UserInfo` |

The structure drawing - before and after - is in `docs/architecture.svg`.

## Everything else

* `cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
  clippy and test jobs are green on the tagged commit.

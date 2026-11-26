Windows 10/11 system components rewritten in Rust. Release 0.9.2 answers a
second review of 0.9.1: four groups of reports, fixed at their causes.

## 1. Explorer

* **A drag is a move.** Dropping a folder into another folder copied it - the
  drop passed `cut = false` unconditionally - while the file job it started was
  labelled "Copying" and left the source in place. A drag now moves (copy then
  delete across volumes) and holding Ctrl copies, which is what the same
  gesture does in Windows.
* **You can see where it will land.** The hovered folder's whole row is filled
  and outlined during the drag; before, only a hairline around the name cell
  hinted at it, and only in the details view.
* **The window opens on "This PC"**, so the drives are the first thing on
  screen. A virtual folder is a tab that carries the marker itself, so opening
  a drive no longer falls back to the drive grid.
* **The left pane is the user's now.** The "folders in this view" list merely
  repeated the right pane; it is gone, and in its place is a favourites list
  filled by dragging folders onto it. Right-click an entry to unpin it. The
  list survives a restart.
* **Rubber-band selection in all four views.** Press on empty space, drag a
  box, release: everything the box touched is selected, and Ctrl adds to an
  existing selection instead of replacing it. The box tests the rows' real
  screen rectangles, recorded by each view as it draws, so it cannot drift from
  what is on screen.

## 2. Start, taskbar and the tray

* **The Start button no longer closes what it just opened.** The taskbar draws
  before the Start menu, so the click that opened the menu was still "a click
  outside it" when the menu's own outside-click test ran later in the same
  frame. The session stamps the frame it opened on, and that frame is exempt.
* **The button is the Windows mark**, four painted panes, not `⊞` (U+229E,
  "squared plus"), which read as a plus in a box on every font.
* **Start > Settings is Windows Settings.** It opened the launcher's own
  options; it opens `ms-settings:`, the same thing `Win+I` does - and `Win+I`
  was not implemented at all, now it is.
* **Flyouts sit on the bar.** They hung from the bar's top edge downward, so
  they covered the taskbar and their lower rows ran off the screen. Their
  bottom edge now sits on the bar's top edge and they grow upward. The overflow
  chevron points up over the panel it opens.
* **The volume slider works.** `with_device()` created its own `ComGuard`, so
  `CoUninitialize` ran while the caller still held the `IMMDevice`; every query
  on it failed and `master_volume()` was always `None`. The guard belongs to
  the function that owns the objects. An unknown level shows "—", never a
  false zero.
* **The running indicator is an underline** pinned to the bar's bottom edge and
  wider for a process with several windows, in place of a dash inside the
  button.
* **Clicking a thumbnail peeks.** Resting the pointer on a taskbar preview
  raises a full-screen peek - the screen dims, that window is shown large -
  and moving off it lowers the peek. A window that closes takes its peek with
  it. Hovering the preview no longer opens it, which is what made the old
  "hover" feel like a jump.

## 3. Task Manager

* **Sorting starts in the direction the column reads.** Every column started
  descending, so the first click on "Name" looked like nothing had happened;
  text columns now start A-Z and numeric columns biggest-first, and a second
  click reverses.
* **The group headers are checkboxes.** "Приложения" and "Фоновые процессы"
  had a bare chevron, which said nothing about whether the group was part of
  the view; a ticked box means shown, and clicking the box or the caption
  toggles it.
* **Update speed is a setting**: 1 s by default, 3 s, 5 s and instant, offered
  by a right-click on the process list. The sampler keeps sampling at its own
  rate; the view adopts a sample when the chosen interval allows it, so a
  slower speed does not starve the graphs.

## 4. Services

* **Right-clicking a service opens a menu**: Start, Stop, Pause, Resume,
  Restart, Properties. Restart is stop, wait, then start, and a service that
  refuses to stop reports that instead of leaving the machine half-restarted.
  The menu acts on the row it was opened on, and opening it selects that row.

## Where the new state lives

The same seams the linter pointed at in 0.9.1, extended rather than duplicated:

| new in 0.9.2 | owner |
| --- | --- |
| the frame a menu was opened on, and the `Win+I` latch | `session::Session` (`opened_at`, `note_opened`, `settings_chord_seen`) |
| how often the view adopts a sample | `app::UpdateInterval`, with `hold()` as the only place its meaning is spelled out |
| which folders the left pane shows | `ExplorerApp::pinned`, saved in `PersistedState` alongside the tab list |
| the rows a rubber band can touch | `ExplorerApp::row_rects`, refilled by each view as it draws |
| a generic key probe | `shell::window::key_down(vk)`, with `win_key_down` / `v_key_down` / `i_key_down` as thin names for it |

`shell::window::key_down` replaces the two near-copies of the
`GetAsyncKeyState` read, so a fourth chord is now a one-line wrapper rather
than a third implementation.

The structure drawing - before and after - is in `docs/architecture.svg`.

## Everything else

* `cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
  build, fmt, clippy and test jobs are green on the tagged commit.
* The volume fix also removes the last `ComGuard` inside `with_device()`, so
  the rule "one COM apartment per top-level operation" now holds everywhere in
  `devices::audio`.

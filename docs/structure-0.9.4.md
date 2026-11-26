# Desktop crate structure: before and after 0.9.4

The 0.9.4 rules say the deslop findings must be fixed from first principles,
and that `module-cycle` / `tangle` need a decision about which module belongs
at which layer. This note records the decision and the resulting structure.

## Before

```
                    +----------------+
                    |      main      |   owns DesktopApp, the shell loop
                    +-------+--------+
                            |
          +-----------------+-----------------+
          |                 |                 |
          v                 v                 v
    +-----------+     +-----------+     +-----------+
    |  session  |<--->|   start   |     |  taskbar  |
    |  (state)  |     |  (view)   |     |  (view)   |
    +-----------+     +-----------+     +-----------+
          |                 |
          |                 |
          +------ both need the same two types ------+
                            |
                     AppEntry, PowerAction
                     defined in session
                            ^
                            |
                  default_pins() defined in start
```

The two boxes point at each other, so as far as the compiler is concerned
`session` and `start` are one module that has been split across two files.
That is the `module-cycle` finding, and it is a layering mistake rather than
a formatting one: a *state owner* was importing a *view*, and a *view* was
importing *state*, for two different reasons.

Field leaks in the same pass:

| Type | Public fields before | What they were |
| ---- | -------------------- | -------------- |
| `DesktopApp` | 44 | included `selected`, written 8x, read 0x |
| `ExplorerApp` | 47 | included 8 raw `mpsc` channels |
| `Session` | 2 | `power_menu_open`, `user_menu_open` |

## After

```
                        +----------------+
                        |      main      |
                        +-------+--------+
                                |
        +-----------+-----------+-----------+-----------+
        |           |           |           |           |
        v           v           v           v           v
  +---------+ +---------+ +---------+ +---------+ +---------+
  |  model  | | session | |  start  | | taskbar | |  tray   |
  | (types) | | (state) | | (view)  | | (view)  | | (view)  |
  +---------+ +---------+ +---------+ +---------+ +---------+
        ^           ^           ^           ^
        |           |           |           |
        +-----------+-----------+-----------+   every module depends
                                                downwards on model
```

`model` is the answer to "which module belongs at which layer": the shared
vocabulary (`AppEntry`, `PowerAction`, `default_pins`) is data, not
behaviour, so it belongs at the bottom, where both the state owner and the
view can import it without pointing sideways at each other. `session` keeps
the state, `start` keeps the drawing, and neither knows the other exists.

The same pass split the one big `Session` into three owners that each hold a
single invariant:

```
  Session                 ClipboardPanel        Chord  (x2)
  ------------------      ------------------    ------------------
  open / search /         open?                 "already seen" latch
  anim / apps / pins      toggle / hide         for Win+V and Win+I
  power + user lists
  (private, with verbs)
```

Narrow interfaces now hold the invariants that public fields used to let
callers break:

* `Session::toggle_power_menu` closes the account list, so the two lists can
  never be open over each other.
* `ExplorerApp::workers` keeps the four queues private; the shell can only
  hand a sender to a thread or drain a receiver, never see a channel.
* `Chord` *is* the latch, so there is no way to call the edge detector and
  forget to reset it.

## Numbers

| Measure | Before | After |
| ------- | ------ | ----- |
| `module-cycle` in `winrust-desktop` | 1 (`session <-> start`) | 0 |
| `Session` public functions | 31 | 29 (`ClipboardPanel` and `Chord` took the other 6) |
| `Session` public fields | 2 | 0 |
| `ExplorerApp` public fields | 47 | 41 (8 channels became 1 `workers`) |
| `DesktopApp` public fields | 44 | 46 (`selected` gone, `ClipboardPanel` + 2 `Chord`s added) |
| `DesktopApp` write-only fields | 1 (`selected`) | 0 |

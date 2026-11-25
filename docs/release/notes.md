Windows 10/11 system components rewritten in Rust. Release 0.9.0 is the
structural release: the whole codebase was run through
[deslop-please](https://github.com/deterministic-machines/deslop-please) and
every finding was fixed at the cause, not suppressed. Nothing looks different on
screen - the point is that the next change is cheaper to make.

## The shell owns its state

* The tray's eight loose fields (`volume`, `muted`, `lang_tag`, `notifications`,
  `tray_flyout`, `tray_overflow`, `notifications_read`, `start_open` all lived
  on `DesktopApp`) are now `TrayState`, a type with private fields. Readers use
  getters; writers use verbs - `toggle` / `open` / `close`, `set_volume`,
  `set_muted`, `set_language`, `clear_notifications`, `mark_read` - and one
  `sample()` runs the audio / network / layout poll on the tray's own cadence.
* `DesktopApp` could not guard a single field of its own, because nothing was
  its own. The invariants ("a tray flyout and the Start menu are never open
  together", "the volume tooltip follows the device") were re-proved by hand at
  every call site; now the type proves them.
* The Start menu got the same treatment: `show_start`, `show_start_power`,
  `toggle_start`, `close_start`.
* `notifications_read`, a field that was only ever set to `0`, is gone.

## The dependency graph has no circles

`system`, `shell` and `process` each used the others, so there was no order to
read or link them in:

* session power (`lock_workstation`, `sign_out`, `restart`, `shutdown`) moved
  from `system` to `shell`, where `launch` lives;
* the top-level window list, their icons, focus and the `Win` key moved from
  `shell` into `core::window` - the only part of `shell` that needed a process
  image;
* `process::set_affinity` reads the processor count for its "all cores" mask
  itself instead of reaching up into `system`.

## Files are split by concern

| was | now |
| --- | --- |
| `explorer/app.rs` 3560 lines | `app.rs` 42 + `viewmode` `tabs` `state` `commands` `panels` `sidebar` `views` `preview` `keyboard` |
| `taskmgr/app.rs` 2184 lines | `app.rs` 273 + `model` `ui` `startup` `format` `sort` `tabs` |
| `core/src` 22 flat modules | `system/` `storage/` `shell/` `devices/` `i18n/` + `error` `handle` `icon` `wide` |

No call site moved when the core was grouped: every old path is a `pub use` at
the crate root, so `winrust_core::process::enumerate()` still compiles and now
resolves to `winrust_core::system::process`.

## What the linter reports

| rule | before | after |
| --- | --- | --- |
| `field-leak` | 8 fields on 1 type | 0 |
| `tangle` | 1 circle (3 modules) | 0 |
| `module-cycle` | 0 | 0 |
| `long-file` (> 2000 lines) | 2 | 0 |
| `wide-directory` (> 15 members) | 1 (22 members) | 0 |

The before / after structure drawing is in `docs/architecture.svg`.

## Everything else

* Two warnings that had been in the tree for a while are gone: an unused
  binding in `fileassoc.rs` and an unnecessary `unsafe` around a union field in
  `gpu.rs`.
* `cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
  clippy and test jobs are green on the tagged commit.

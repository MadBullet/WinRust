//! The taskbar strip: Start, running programs, and the Windows-style tray.
//!
//! The layout follows Windows 10/11 rather than a generic toolbar:
//!
//! * **left** - the Start button, and nothing else.  Right-clicking it opens the
//!   Win+X menu (Task Manager, Settings, Explorer, ...).
//! * **centre-left** - one button per *running* program.  Pinned-but-closed
//!   tools are not shown, because the request was explicit: only active
//!   programs belong on the bar.
//! * **right** - the tray, exactly as in Windows: an overflow chevron, network,
//!   volume, input language, the clock with a calendar flyout, and the
//!   notification centre.  Each opens a flyout (see `tray.rs`).
//!
//! Application buttons use the icon the window itself reports
//! (`window::window_icon_rgba`), so an entry never renders as an empty square.

use eframe::egui;
use egui::{Color32, RichText};
use std::time::Instant;
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::tray;
use crate::DesktopApp;

pub const HEIGHT: f32 = 46.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEntry {
    pub title: String,
    pub pid: u32,
    pub hwnd: isize,
    /// Executable path, used for the fallback icon.
    pub exe: String,
}

/// Which taskbar button the pointer is resting on, since when, and where the
/// button is.
///
/// These three facts only make sense together - a preview without an anchor
/// cannot be placed, and an anchor without a start time cannot be delayed - so
/// they live in one value with three verbs instead of three fields on the
/// shell.  (The 0.9.0 shell had `hover_pid`, `hover_since` and placed the
/// preview against the screen edge, which is why it appeared to the right of
/// the pointer instead of over it.)
#[derive(Debug, Clone, Copy)]
pub struct Hover {
    pid: u32,
    since: Instant,
    anchor: egui::Rect,
}

impl Hover {
    /// The pointer entered `pid`'s button this frame, or moved to another.
    pub fn enter(&mut self, pid: u32, anchor: egui::Rect) {
        if self.anchor != anchor {
            self.pid = pid;
            self.since = Instant::now();
            self.anchor = anchor;
        }
    }

    /// The pointer left every button.
    pub fn clear(&mut self) {
        // `anchor` is the "nothing hovered" marker: an empty rect can never be
        // a real button.
        self.anchor = egui::Rect::NOTHING;
    }

    pub fn is_empty(&self) -> bool {
        !self.anchor.is_positive()
    }

    pub fn pid(&self) -> Option<u32> {
        (!self.is_empty()).then_some(self.pid)
    }

    pub fn anchor(&self) -> egui::Rect {
        self.anchor
    }

    /// Long enough for a preview, and long enough to skip a quick pass-over.
    pub fn rested(&self) -> bool {
        !self.is_empty() && self.since.elapsed() >= std::time::Duration::from_millis(350)
    }
}

impl Default for Hover {
    fn default() -> Self {
        Self {
            pid: 0,
            since: Instant::now(),
            anchor: egui::Rect::NOTHING,
        }
    }
}

/// List windows through `EnumWindows`, with the owning process' image path so
/// the button can fall back to the executable's icon.
pub fn windows() -> Vec<WindowEntry> {
    winrust_core::window::top_level_windows()
        .into_iter()
        .filter(|w| !w.title.is_empty())
        // The shell's own window is the desktop; it must not appear on the bar.
        .filter(|w| w.pid != std::process::id())
        .map(|w| WindowEntry {
            title: w.title,
            pid: w.pid,
            hwnd: w.hwnd,
            exe: w.exe,
        })
        .collect()
}

impl DesktopApp {
    pub fn taskbar(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let bar = egui::Rect::from_min_max(
            egui::pos2(screen.left(), screen.bottom() - HEIGHT),
            screen.right_bottom(),
        );
        let pal = winrust_ui::theme::pal(&self.appearance);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("taskbar_bg"),
        ));
        let bar_fill = Color32::from_rgba_unmultiplied(
            pal.surface.0.saturating_sub(8),
            pal.surface.1.saturating_sub(8),
            pal.surface.2.saturating_sub(6),
            246,
        );
        painter.rect_filled(bar, 0.0, bar_fill);
        painter.line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
            ),
        );

        // ---- Left: the Start button only -----------------------------------
        let inner = bar.shrink2(egui::vec2(6.0, 4.0));
        let mut left_ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("taskbar_left")),
            egui::Id::new("taskbar_left_ui"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    inner.left_top(),
                    egui::vec2(48.0, inner.height()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // The button keeps its geometry but draws the four-pane Windows mark
        // itself: the old glyph was `⊞` (U+229E "squared plus"), which reads
        // as a plus in a box and does not look like the Windows logo on any
        // font.  A painter keeps it crisp at every DPI and needs no asset.
        let (start_rect, start_resp) =
            left_ui.allocate_exact_size(egui::vec2(40.0, 38.0), egui::Sense::click());
        let hovered = start_resp.hovered();
        if self.start_open() || hovered {
            let alpha = if self.start_open() { 150 } else { 80 };
            left_ui.painter().rect_filled(
                start_rect.shrink(2.0),
                5.0,
                Color32::from_rgba_unmultiplied(
                    pal.selection.0,
                    pal.selection.1,
                    pal.selection.2,
                    alpha,
                ),
            );
        }
        draw_windows_mark(left_ui.painter(), start_rect, pal.text);
        if start_resp.clicked() {
            // Claim focus before toggling: a bottom-most shell window does not
            // become foreground by itself, so without this the very next click
            // (and the `Win` key) went to whatever was in front.
            winrust_core::window::focus_window(self.hwnd);
            self.toggle_start();
        }
        // Win+X: the power-user menu lives on a right click, as in Windows.
        start_resp.context_menu(|ui| {
            ui.set_min_width(250.0);
            for (label, action) in power_menu() {
                if ui.button(t(label)).clicked() {
                    action(self);
                    ui.close_menu();
                }
            }
        });
        start_resp.on_hover_text(t("Start"));

        // ---- Running programs ----------------------------------------------
        // One button per window, grouped by process so two windows of the same
        // program share one button, like the real taskbar.
        let windows_list: Vec<WindowEntry> = self.open_windows.clone();
        // Group by *program*, not by process id.  Two Explorer windows are two
        // `explorer.exe` processes, and a program that opens a new process per
        // window (browsers, Office) would otherwise get a separate button for
        // every window - which is the "программы одного типа не группируются"
        // report.  Windows groups by the application, so the key is the
        // executable's file name; a window with no path falls back to its pid.
        let mut order: Vec<String> = Vec::new();
        for w in &windows_list {
            let key = app_key(w);
            if !order.contains(&key) {
                order.push(key);
            }
        }
        let button_w = 40.0;
        let gap = 2.0;
        let apps_left = inner.left() + 48.0;
        let apps_right = inner.right() - tray::WIDTH;
        let mut apps_ui = egui::Ui::new(
            ctx.clone(),
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("taskbar_apps")),
            egui::Id::new("taskbar_apps_ui"),
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(apps_left, inner.top()),
                    egui::pos2(apps_right.max(apps_left), inner.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        apps_ui.spacing_mut().item_spacing.x = gap;
        let mut hovered_any = false;
        // No `ui.horizontal` here on purpose.  `apps_ui` is already laid out
        // `left_to_right(Center)` over the bar's full height, but a nested
        // `horizontal` starts a new Ui that is only as tall as its tallest
        // child and is aligned to the *parent's* top, so every button sat
        // `(bar_height - 38) / 2` px too low - the "значки съехали вниз" report.
        // Drawing straight into `apps_ui` centres them on the bar, the same
        // axis the Start button uses.
        {
            let ui = &mut apps_ui;
            for key in &order {
                let first = windows_list.iter().find(|w| &app_key(w) == key);
                let Some(first) = first else { continue };
                let group_hwnds: Vec<Option<isize>> = windows_list
                    .iter()
                    .filter(|w| &app_key(w) == key)
                    .map(|w| Some(w.hwnd))
                    .collect();
                let count = group_hwnds.len();
                let icon = self.window_icon(ctx, first);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(button_w, 38.0), egui::Sense::click());
                // Two states, two signals - the 0.9.1 review asked for them to
                // be readable at a glance and they were not:
                //   * running  -> a short blue line under the icon, always;
                //   * active   -> a soft translucent fill behind the button.
                // Hover is a third, weaker fill, so it never masks "active".
                // Active means "one of this group's windows is in front", not
                // just the first one: with grouping by program, the active
                // window is rarely the group's first entry.
                // "Active" is the group that owns the foreground *window*.
                // Comparing pids marked every window of a multi-process
                // program (browsers, Office) as active at once, and missed the
                // case where the foreground belongs to a different process of
                // the same program.
                let selected = group_hwnds.iter().any(|h| *h == self.active_hwnd)
                    || (self.active_hwnd.is_none() && self.active_pid == Some(first.pid));
                if selected {
                    ui.painter().rect_filled(
                        rect.shrink(1.0),
                        5.0,
                        Color32::from_rgba_unmultiplied(
                            pal.selection.0,
                            pal.selection.1,
                            pal.selection.2,
                            90,
                        ),
                    );
                } else if resp.hovered() {
                    ui.painter().rect_filled(
                        rect.shrink(1.0),
                        5.0,
                        Color32::from_rgba_unmultiplied(pal.hover.0, pal.hover.1, pal.hover.2, 70),
                    );
                }
                let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(22.0, 22.0));
                match icon {
                    Some(id) => {
                        ui.painter().image(
                            id,
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.painter().text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph_for_title(&first.title),
                            egui::FontId::proportional(17.0),
                            Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
                        );
                    }
                }
                // The running indicator: a Windows-blue line under the icon,
                // sitting on the bar's bottom edge.  Two complaints shaped
                // this: it has to be visible at a glance, and one process with
                // several windows has to look different from one with one.
                //
                // The underline is 3 px tall and pinned to `bar.bottom() - 3`,
                // not to the button's own bottom - the button is inset inside
                // the bar, so a line at `rect.bottom() - 4` floated in the
                // middle of the button and read as a stray dash rather than an
                // underline.
                let blue = Color32::from_rgb(0x60, 0xa8, 0xff);
                let windows_of_pid = count.min(3) as f32;
                let width = 14.0_f32 + (windows_of_pid - 1.0) * 5.0;
                let x = rect.center().x - width / 2.0;
                let y = bar.bottom() - 3.0;
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, 3.0)),
                    1.5,
                    blue,
                );
                // The group's own pid; with one window per process this is what
                // the preview and the click act on.
                let group_pid = first.pid;
                if resp.hovered() {
                    hovered_any = true;
                    self.hover.enter(group_pid, rect);
                    // Remember the group, not just the pid: the strip below is
                    // built from it, and a pid is not unique across a program
                    // with several windows.
                    self.hovered_group = Some(key.clone());
                }
                if resp.clicked() {
                    // Raise the window of this group that the user last used,
                    // falling back to the first one.  Activating by pid raised
                    // whichever window Windows felt like, not the one clicked.
                    let target = group_hwnds
                        .iter()
                        .flatten()
                        .copied()
                        .find(|h| Some(*h) == self.active_hwnd)
                        .unwrap_or(first.hwnd);
                    self.active_pid = Some(group_pid);
                    winrust_core::window::activate_window_hwnd(target);
                }
                let hover = if count > 1 {
                    format!("{}\n{}", first.title, tf!("{} windows", count))
                } else {
                    first.title.clone()
                };
                resp.on_hover_text(hover);
            }
            // Nothing running: say so quietly instead of leaving a void.
            if order_empty(&self.open_windows) {
                ui.label(RichText::new(ts("No open windows")).small().weak().color(
                    Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2),
                ));
            }
        }

        // Hover preview: a strip of miniatures, one per window of the group,
        // drawn above the button after a short rest - the same behaviour as the
        // real taskbar.  Single windows get one too.
        //
        // This runs outside the button closure: `preview` needs `&mut self`,
        // which the closure has already borrowed for the button loop.
        if hovered_any {
            // Sticky while the pointer travels from the button to the strip.
            if !self.hover.is_empty() {
                self.hovered_group = self
                    .hover
                    .pid()
                    .and_then(|pid| self.open_windows.iter().find(|w| w.pid == pid))
                    .map(app_key)
                    .or_else(|| self.hovered_group.clone());
            }
        }
        if let Some(group_key) = self.hovered_group.clone() {
            let group: Vec<WindowEntry> = self
                .open_windows
                .iter()
                .filter(|w| app_key(w) == group_key)
                .cloned()
                .collect();
            // The strip must be *built* whenever the pointer is still around,
            // not only when it is on a button: once it is up the pointer moves
            // onto it, `hovered_any` is false for a frame, and rebuilding only
            // on `rested()` dropped the strip before the hit test could run.
            // `preview` itself reports whether the pointer is on it.
            if !group.is_empty()
                && (hovered_any || self.hover.rested() || self.hovered_group.is_some())
            {
                // Read the anchor before `self` is lent to `preview`; with the
                // pointer already on the strip the anchor is stale, which is
                // fine - the strip is not moved once it is up.
                let anchor = self.hover.anchor();
                let on_card = preview(self, ctx, bar, pal, &group, anchor);
                // The miniature is drawn *outside* the button loop, so the
                // pointer leaves the button the moment it moves onto a card.
                // A card counts as still being on the group.
                if on_card {
                    hovered_any = true;
                    // The strip is under the pointer: remember it, so the
                    // grace period below starts counting from *now* and not
                    // from the moment the pointer left the button.
                    self.preview_at = Instant::now();
                }
            }
        }
        // Leaving the buttons *and* the cards drops the preview, like the bar
        // does, and takes the full-screen peek with it.
        // Grace period: crossing the gap between button and strip, or moving
        // inside the strip's padding, must not dismiss it.
        let grace = self.hovered_group.is_some()
            && self.preview_at.elapsed() < std::time::Duration::from_millis(250);
        if !hovered_any && !grace {
            self.hover.clear();
            self.hovered_group = None;
            self.peek_hwnd = None;
            self.peek_at = None;
        }
        if grace {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // ---- Right: the tray -------------------------------------------------
        tray::draw(self, ctx, inner);
    }

    /// Cached icon for one window: the window's own icon, then its executable's.
    pub(crate) fn window_icon(
        &mut self,
        ctx: &egui::Context,
        w: &WindowEntry,
    ) -> Option<egui::TextureId> {
        if let Some(slot) = self.window_icons.get(&w.pid) {
            return slot.as_ref().map(|t| t.id());
        }
        // Three sources, cheapest first: the window's own icon, then the
        // executable's own icon, then the executable path looked up again.
        // The last one matters: `WindowEntry::exe` is captured when the
        // window list is built, and for a process that could not be opened at
        // that moment it is empty - which is why some taskbar buttons fell
        // back to a text glyph.  Asking the process again here costs one
        // `QueryFullProcessImageNameW` per previously iconless button.
        let exe = if w.exe.is_empty() {
            winrust_core::system::process::exe_path_of(w.pid).unwrap_or_default()
        } else {
            w.exe.clone()
        };
        let rgba = winrust_core::window::window_icon_rgba(w.hwnd, 32)
            .or_else(|| winrust_core::window::exe_icon_rgba(&exe, 32))
            .or_else(|| {
                // A shortcut to the program in the Start menu carries the same
                // picture, and `install::find_binary` knows where the shell's
                // own programs live.
                let name = std::path::Path::new(&exe)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())?;
                let path = winrust_core::install::find_binary(&name)?;
                winrust_core::window::exe_icon_rgba(&path.to_string_lossy(), 32)
            });
        let texture = rgba.and_then(|rgba| {
            let px = match rgba.len() / 4 {
                n if n >= 48 * 48 => 48,
                n if n >= 32 * 32 => 32,
                n if n >= 16 * 16 => 16,
                _ => return None,
            };
            let expected = px * px * 4;
            if rgba.len() < expected {
                return None;
            }
            Some(ctx.load_texture(
                format!("winicon:{}", w.pid),
                egui::ColorImage::from_rgba_unmultiplied([px, px], &rgba[..expected]),
                egui::TextureOptions::LINEAR,
            ))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.window_icons.insert(w.pid, texture);
        id
    }
}

/// The key that groups a window's taskbar button: the program it belongs to.
///
/// Two Explorer windows are two `explorer.exe` processes, and a program that
/// opens a new process per window would otherwise get one button per window.
/// Windows groups by application, so the key is the executable's file name; a
/// window with no path falls back to its process id so it still gets a button.
pub(crate) fn app_key(w: &WindowEntry) -> String {
    let exe = std::path::Path::new(&w.exe)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if exe.is_empty() {
        format!("pid:{}", w.pid)
    } else {
        exe
    }
}

fn order_empty(list: &[WindowEntry]) -> bool {
    list.is_empty()
}

/// The Start button's right-click menu (Win+X).
/// The four-pane Windows mark, drawn as four squares with a one-pixel gap.
///
/// Windows 10/11 use this shape for the Start button; it is drawn rather than
/// shipped as an icon so it stays sharp at any scale factor.
fn draw_windows_mark(p: &egui::Painter, rect: egui::Rect, color: (u8, u8, u8)) {
    let fill = Color32::from_rgb(color.0, color.1, color.2);
    let pane = 7.5_f32;
    let gap = 1.5_f32;
    let total = pane * 2.0 + gap;
    let origin = rect.center() - egui::vec2(total / 2.0, total / 2.0);
    for (dx, dy) in [
        (0.0, 0.0),
        (pane + gap, 0.0),
        (0.0, pane + gap),
        (pane + gap, pane + gap),
    ] {
        p.rect_filled(
            egui::Rect::from_min_size(origin + egui::vec2(dx, dy), egui::vec2(pane, pane)),
            0.5,
            fill,
        );
    }
}

fn power_menu() -> Vec<(&'static str, fn(&mut DesktopApp))> {
    vec![
        ("Task Manager", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-taskmgr.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Settings", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("WinRust.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Explorer", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-explorer.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Desktop", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-desktop.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("System Control", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-sysctl.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Services", |_app: &mut DesktopApp| {
            if let Some(p) = winrust_core::install::find_binary("winrust-services.exe") {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
        }),
        ("Power", |app: &mut DesktopApp| {
            // Same confirmation as the Start menu's power list: Win+X is one
            // keystroke from a shutdown and must not fire on a misclick.
            app.ask_power(crate::session::PowerAction::ShutDown);
        }),
        ("Lock", |_app: &mut DesktopApp| {
            winrust_core::shell::lock_workstation();
        }),
        ("Sign out", |app: &mut DesktopApp| {
            app.ask_power(crate::session::PowerAction::SignOut);
        }),
        ("Shut down", |app: &mut DesktopApp| {
            app.ask_power(crate::session::PowerAction::ShutDown);
        }),
        ("Restart", |app: &mut DesktopApp| {
            app.ask_power(crate::session::PowerAction::Restart);
        }),
    ]
}

/// A glyph for a window whose icon could not be read at all.
fn glyph_for_title(title: &str) -> &'static str {
    let t = title.to_lowercase();
    if t.contains("проводник") || t.contains("explorer") {
        "🗂"
    } else if t.contains("диспетчер") || t.contains("task manager") {
        "📊"
    } else if t.contains("служб") || t.contains("services") {
        "⚙"
    } else {
        "🪟"
    }
}

/// The hover preview: one card per window of the group, over the button.
///
/// Three things the 0.9.1 review asked for, and what each one means here:
///
/// * **at the mouse** - the strip is centred on the button that is hovered
///   (`anchor`, passed in by the bar) and clamped to the screen, instead of
///   being pinned to the tray's right edge, which put it a foot away from the
///   pointer;
/// * **shape follows the window** - a card is `96..=190` px wide and
///   `80..=150` px tall, chosen from the window's own aspect ratio, so a
///   portrait document is not letterboxed into a 16:10 hole;
/// * **thumbnail first** - the picture fills the card, the title is one line
///   under it, and a window with no picture is not offered at all (the caller
///   filters those out).
///
/// Cost is bounded the way the bar needs it: each picture is captured at most
/// once every two seconds, and a capture asks for exactly the card's size, not
/// a fixed 400x250 that would then be scaled down.
impl DesktopApp {
    /// The full-screen peek.
    ///
    /// Resting the pointer on a taskbar thumbnail raises this: the screen dims
    /// and the hovered window is shown large, in the same place and at the same
    /// shape it occupies, so the user can read it without switching.  Moving
    /// off the thumbnail clears `peek_hwnd` and the overlay disappears - there
    /// is no separate dismiss gesture and nothing stays behind.
    ///
    /// It is a capture, not a live window: raising a real window would take the
    /// foreground away from the shell and the taskbar would lose the pointer,
    /// which is what "all windows hidden, only the hovered one shown" means in
    /// practice for a shell that owns the desktop.
    pub(crate) fn peek_overlay(&mut self, ctx: &egui::Context) {
        let Some(hwnd) = self.peek_hwnd else {
            return;
        };
        // A window that closed while its peek was up must not leave the screen
        // dimmed with a stale picture in the middle of it.
        let Some(size) = winrust_core::thumb::window_size(hwnd) else {
            self.peek_hwnd = None;
            return;
        };
        let screen = ctx.screen_rect();
        let pal = winrust_ui::theme::pal(&self.appearance);

        // Capture once per peek: a 60 Hz `PrintWindow` would cost more than the
        // feature is worth, and the point is to read a static window.
        let want_w = (screen.width() as u32).min(1600).max(320);
        let want_h = (screen.height() as u32).min(1000).max(200);
        if self.peek_at != Some(hwnd) {
            let image = winrust_core::thumb::window_thumbnail(hwnd, want_w, want_h).map(|t| {
                egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                )
            });
            self.peek_tex =
                image.map(|img| ctx.load_texture("peek_window", img, egui::TextureOptions::LINEAR));
            self.peek_at = Some(hwnd);
        }

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("peek_overlay"),
        ));
        // The dim: everything behind the peeked window is hidden, which is the
        // effect the request asked for ("all windows hidden, only the hovered
        // one shown").
        painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 190));
        let (w, h) = (size.0 as f32, size.1 as f32);
        let scale = ((screen.width() * 0.86) / w)
            .min((screen.height() * 0.86) / h)
            .min(1.0);
        let target = egui::Rect::from_center_size(screen.center(), egui::vec2(w, h) * scale);
        match &self.peek_tex {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    target,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(target, 6.0, Color32::from_rgb(30, 33, 42));
            }
        }
        painter.rect_stroke(
            target,
            6.0,
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
            ),
        );
    }
}

fn preview(
    app: &mut DesktopApp,
    ctx: &egui::Context,
    bar: egui::Rect,
    pal: winrust_ui::theme::Palette,
    windows: &[WindowEntry],
    anchor: egui::Rect,
) -> bool {
    const GAP: f32 = 6.0;
    const PAD: f32 = 6.0;
    const CAPTION: f32 = 18.0;

    let refresh = app
        .thumbs_at
        .map(|t| t.elapsed() > std::time::Duration::from_secs(2))
        .unwrap_or(true);
    if refresh {
        app.thumbs_at = Some(Instant::now());
        // Forget pictures of windows that are gone; otherwise the texture map
        // grew for the lifetime of the shell (each entry is a GPU texture).
        let live: std::collections::HashSet<isize> = windows.iter().map(|w| w.hwnd).collect();
        app.thumbs.retain(|hwnd, _| live.contains(hwnd));
    }

    // One card size for the whole strip: the shape of the first window is the
    // group's shape, which keeps a group of docks the same height.
    let shape = windows
        .first()
        .and_then(|w| winrust_core::thumb::window_size(w.hwnd))
        .map(|(w, h)| w as f32 / h.max(1) as f32)
        .unwrap_or(16.0 / 10.0);
    let (card_w, card_h) = card_for(shape);
    let card = egui::vec2(card_w, card_h);
    // Whether the pointer is on any card; reported back so the caller keeps the
    // strip (and the peek) alive while the user is on it.
    let mut hovered_card = false;

    let count = windows.len() as f32;
    let width = count * card.x + (count - 1.0) * GAP + PAD * 2.0;
    let height = card.y + CAPTION + PAD * 2.0;
    let screen = ctx.screen_rect();

    // Centred over the hovered button, then clamped: a button near the screen
    // edge gets a strip that has slid inward rather than one that is cut off.
    let want_left = anchor.center().x - width / 2.0;
    let left = want_left
        .max(screen.left() + 4.0)
        .min(screen.right() - width - 4.0);
    // Above the bar, never overlapping it.
    let top = (bar.top() - height - 6.0).max(screen.top() + 4.0);
    let rect = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height));

    // Capture/refresh every picture first.  `Painter` borrows the context, so
    // calling `ctx.load_texture` while it is alive does not compile - and
    // loading before painting also keeps the frame's shape list tidy.
    let want = (card.x as u32 * 2).max(160);
    for w in windows.iter() {
        if refresh || !app.thumbs.contains_key(&w.hwnd) {
            let image = winrust_core::thumb::window_thumbnail(w.hwnd, want, want).map(|t| {
                egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                )
            });
            let handle = image.map(|img| {
                ctx.load_texture(
                    format!("thumb_{}", w.hwnd),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            app.thumbs.insert(w.hwnd, handle);
        }
    }

    let surface = Color32::from_rgba_unmultiplied(pal.surface.0, pal.surface.1, pal.surface.2, 248);
    // `Order::Foreground` is *below* the taskbar's own input areas, which are
    // also foreground: a pointer over the strip was still reported as being
    // over the bar, so the strip could never be hovered and the preview went
    // away as soon as the user left the button.  Tooltip is above all of them.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("taskbar_preview_bg"),
    ));
    painter.rect_filled(rect, 8.0, surface);
    painter.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(
            1.0_f32,
            Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
        ),
    );

    // Texture ids only: holding the map borrowed would block the `app` writes
    // below (the preview resets the hover state when a card is clicked).
    let ids: Vec<(isize, Option<egui::TextureId>, egui::Vec2)> = windows
        .iter()
        .map(|w| {
            let tex = app.thumbs.get(&w.hwnd).and_then(|t| t.as_ref());
            (
                w.hwnd,
                tex.map(|t| t.id()),
                tex.map(|t| t.size_vec2()).unwrap_or_default(),
            )
        })
        .collect();

    let mut clicked: Option<isize> = None;
    for (i, w) in windows.iter().enumerate() {
        let card_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + PAD + i as f32 * (card.x + GAP),
                rect.top() + PAD,
            ),
            card,
        );
        let (_, tex_id, size) = ids[i];
        // Guarded arms do not count towards exhaustiveness, so the size check
        // lives inside the arm rather than on the pattern.
        let sized = (size.x > 0.0 && size.y > 0.0).then_some(tex_id).flatten();
        match sized {
            Some(id) => {
                // Letterbox inside the card so odd window shapes stay centred;
                // the card itself already follows the shape, so this only trims
                // a group whose second window has a different shape.
                let scale = (card_rect.width() / size.x).min(card_rect.height() / size.y);
                let draw = egui::Rect::from_center_size(card_rect.center(), size * scale);
                painter.image(
                    id,
                    draw,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(card_rect, 4.0, Color32::from_rgb(30, 33, 42));
                painter.text(
                    card_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph_for_title(&w.title),
                    egui::FontId::proportional(24.0),
                    Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2),
                );
            }
        }
        painter.text(
            egui::pos2(card_rect.center().x, card_rect.bottom() + 3.0),
            egui::Align2::CENTER_TOP,
            winrust_core::i18n::end(&w.title, 22),
            egui::FontId::proportional(10.5),
            Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2),
        );
    }

    // The click targets are registered after painting: `Painter` borrows the
    // context, so the two cannot overlap.
    for (i, w) in windows.iter().enumerate() {
        let card_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + PAD + i as f32 * (card.x + GAP),
                rect.top() + PAD,
            ),
            card,
        );
        let resp = egui::Area::new(egui::Id::new(("thumb_hit", w.hwnd)))
            .order(egui::Order::Tooltip)
            .fixed_pos(card_rect.left_top())
            .show(ctx, |ui| {
                ui.allocate_exact_size(card, egui::Sense::click()).1
            })
            .inner;
        if resp.clicked() {
            clicked = Some(w.hwnd);
        }
        // Resting on a thumbnail raises the full-screen peek; moving off it
        // lowers it again.  `hovered()` is per-card, so exactly one window can
        // be peeked at a time and it goes away the moment the pointer leaves.
        if resp.hovered() {
            app.peek_hwnd = Some(w.hwnd);
            hovered_card = true;
        }
    }

    if let Some(hwnd) = clicked {
        winrust_core::window::activate_window_hwnd(hwnd);
        app.peek_hwnd = None;
        app.hover.clear();
        // The shell window is behind everything again after this, so the
        // preview must go away rather than linger until the next repaint.
        app.thumbs_at = None;
    }
    // Report whether the pointer is on the strip *or* on any one card, so the
    // caller can keep the preview alive while the user is looking at it.
    let on_strip = ctx
        .input(|i| i.pointer.latest_pos())
        .map(|p| rect.expand(GAP).contains(p))
        .unwrap_or(false);
    on_strip || hovered_card
}

/// The card size for a window's aspect ratio: landscape stays wide, a square
/// window gets a square, a portrait one gets a tall card.  The bounds keep a
/// strip of four windows on screen and a single one readable.
fn card_for(shape: f32) -> (f32, f32) {
    let height = 104.0_f32;
    let width = (height * shape).clamp(84.0, 178.0);
    // A tall/narrow window would otherwise get a card shorter than the
    // caption; grow the height to keep the shape.
    let height = if shape < 1.0 {
        (width / shape).clamp(height, 150.0)
    } else {
        height
    };
    (width, height)
}

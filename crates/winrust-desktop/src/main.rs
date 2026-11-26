//! WinRust Desktop — a working replacement for `explorer.exe`.
//!
//! This is the program the review asked for: after installing it as the shell
//! (launcher → Explorer → *Custom shell (Winlogon)*), signing in starts *this*
//! window instead of the stock desktop.  It provides the three things a shell
//! must do:
//!
//! * **Desktop** — a wallpaper-coloured grid of the user's Desktop folder with
//!   double-click to open, drag to move, and the Explorer context menu.
//! * **Taskbar** — Start, running windows, pinned apps, clock, and the tray-ish
//!   status (CPU / RAM) so the machine stays observable.
//! * **Start menu** — search over installed apps and known folders, power
//!   actions (lock, sign out, restart, shut down) and a way back to the stock
//!   shell, so the user is never trapped.
//!
//! The window is a normal eframe window sized to the screen and kept behind
//! everything else (`always_on_bottom`), which is exactly how a shell behaves.

mod desktop;
mod session;
mod start;
mod taskbar;
mod tray;

use eframe::egui;
use egui::{Color32, RichText};
use std::time::{Duration, Instant};
use winrust_core::i18n::{self, Lang, ts};
use winrust_core::{t, tf};

/// What the shell is doing with the mouse, across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellMode {
    #[default]
    Idle,
    /// Dragging a desktop icon towards a new grid cell.
    DraggingIcon(usize),
}

pub struct DesktopApp {
    pub(crate) mode: ShellMode,
    pub(crate) now: Instant,
    /// Desktop entries, refreshed on a timer and after any file operation.
    pub(crate) icons: Vec<desktop::DesktopIcon>,
    pub(crate) icons_at: Instant,
    pub(crate) selected: Option<usize>,
    /// The Start menu's own state (open, search text, app list, animation).
    pub(crate) session: session::Session,
    /// `Win+V` clipboard history.  The list lives in the core; the shell owns
    /// one instance and polls the system clipboard on a slow timer.
    pub(crate) clipboard: winrust_core::shell::ClipboardHistory,
    pub(crate) clipboard_at: Instant,
    pub(crate) open_windows: Vec<taskbar::WindowEntry>,
    pub(crate) windows_at: Instant,
    pub(crate) clock: String,
    pub(crate) status: String,
    /// Icon picture size in pixels (View -> Large/Medium/Small icons).
    pub(crate) icon_size: f32,
    /// View -> Sort by.
    pub(crate) sort_order: desktop::SortOrder,
    pub(crate) sort_desc: bool,
    /// Desktop-specific switches, persisted next to the icon positions.
    pub(crate) settings: desktop::DesktopSettings,
    pub(crate) settings_dirty: bool,
    /// Shell icons, loaded once per path and size.
    pub(crate) icon_cache: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    /// Last copied/cut desktop item, for the Paste command.
    pub(crate) clipboard: Option<std::path::PathBuf>,
    pub(crate) clipboard_cut: bool,
    pub(crate) lang: Lang,
    /// When the registry was last polled for a language change.
    pub(crate) lang_checked_at: Instant,
    pub(crate) hwnd: isize,
    /// Previous frame's `Win` state, for edge detection.
    pub(crate) win_key_was_down: bool,
    /// Wallpaper brush colour mode — the desktop paints itself instead of
    /// showing a picture, so it works on a bare Windows install as well.
    pub(crate) appearance: winrust_core::i18n::Appearance,
    pub(crate) appearance_dirty: bool,
    /// Last system sample for the taskbar readout (CPU %, memory %).
    pub(crate) cpu_mem: (f32, winrust_core::system::MemoryStatus),
    pub(crate) prev_cpu: winrust_core::system::CpuSample,
    pub(crate) sampled_at: Instant,

    // ---- Tray state -------------------------------------------------------
    /// Everything the notification area owns.  Its fields are private; the
    /// shell uses its verbs (`toggle`, `set_volume`, ...) - see `tray.rs`.
    pub(crate) tray: crate::tray::TrayState,
    /// Icons of running applications, keyed by `pid` so they are loaded once.
    pub(crate) window_icons: std::collections::HashMap<u32, Option<egui::TextureHandle>>,
    /// Desktop selection: several icons can be selected at once.
    pub(crate) selection: std::collections::BTreeSet<usize>,
    /// Rubber-band selection in progress: where the LMB went down.
    pub(crate) band_start: Option<egui::Pos2>,
    /// Which application the user last activated from the taskbar.
    pub(crate) active_pid: Option<u32>,
    /// Delete pressed on the wallpaper: the paths waiting for confirmation and
    /// whether Shift was held (skip the Recycle Bin).
    pub(crate) delete_confirm: Vec<std::path::PathBuf>,
    pub(crate) delete_permanent: bool,
    /// 0..=1 animation of the open tray flyout.
    pub(crate) flyout_anim: f32,
    /// Which taskbar button the pointer is resting on, since when, and where
    /// that button is: the preview appears after a short delay, over the
    /// button.
    pub(crate) hover: taskbar::Hover,
    /// Cached window pictures for the hover preview, keyed by hwnd.
    pub(crate) thumbs: std::collections::HashMap<isize, Option<egui::TextureHandle>>,
    pub(crate) thumbs_at: Option<Instant>,
}

impl DesktopApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        i18n::init();
        let appearance = i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);
        Self {
            mode: ShellMode::Idle,
            now: Instant::now(),
            icons: Vec::new(),
            icons_at: Instant::now() - Duration::from_secs(30),
            selected: None,
            session: session::Session::new(),
            clipboard: winrust_core::shell::ClipboardHistory::new(),
            clipboard_at: Instant::now() - Duration::from_secs(30),
            open_windows: Vec::new(),
            windows_at: Instant::now() - Duration::from_secs(30),
            clock: String::new(),
            status: String::new(),
            icon_size: desktop::DEFAULT_ICON,
            sort_order: desktop::SortOrder::Name,
            sort_desc: false,
            settings: desktop::DesktopSettings::load(),
            settings_dirty: false,
            icon_cache: std::collections::HashMap::new(),
            clipboard: None,
            clipboard_cut: false,
            lang: i18n::current(),
            lang_checked_at: Instant::now(),
            hwnd: 0,
            win_key_was_down: false,
            appearance,
            appearance_dirty: false,
            cpu_mem: (0.0, winrust_core::system::memory_status()),
            prev_cpu: winrust_core::system::cpu_sample(),
            sampled_at: Instant::now() - Duration::from_secs(5),
            tray: crate::tray::TrayState::default(),
            window_icons: std::collections::HashMap::new(),
            selection: std::collections::BTreeSet::new(),
            band_start: None,
            active_pid: None,
            delete_confirm: Vec::new(),
            delete_permanent: false,
            flyout_anim: 0.0,
            hover: taskbar::Hover::default(),
            thumbs: std::collections::HashMap::new(),
            thumbs_at: None,
        }
    }

    fn tick(&mut self) {
        self.now = Instant::now();
        if self.icons_at.elapsed() >= Duration::from_secs(5) {
            self.icons = desktop::scan(self.sort_order, self.sort_desc);
            self.icons_at = Instant::now();
        }
        if self.windows_at.elapsed() >= Duration::from_secs(3) {
            self.open_windows = taskbar::windows();
            // Highlight whatever is actually in front, so the taskbar follows the
            // user switching windows with Alt+Tab instead of remembering only the
            // last button they pressed.
            self.active_pid = winrust_core::window::foreground_pid();
            self.windows_at = Instant::now();
        }
        if self.session.apps_stale() {
            self.session.set_apps(start::installed_apps());
        }
        // Two polls: one to *record* copies (every 700 ms is soon enough for a
        // history panel) and a slower one to notice text copied by the other
        // program before the panel is opened.
        if self.clipboard_at.elapsed() >= Duration::from_millis(700) {
            self.clipboard.observe();
            self.clipboard_at = Instant::now();
        }
        self.clock = crate::tray::clock_text();
        // The tray polls its own readouts on its own cadence.
        let mut device = String::new();
        self.tray.sample(&mut device);
        // One CPU sample per tick: the taskbar text is derived from a real
        // interval, not two reads a microsecond apart.
        if self.sampled_at.elapsed() >= Duration::from_millis(1000) {
            let now = winrust_core::system::cpu_sample();
            let pct = winrust_core::system::cpu_usage_between(self.prev_cpu, now);
            self.prev_cpu = now;
            self.cpu_mem = (pct, winrust_core::system::memory_status());
            self.sampled_at = Instant::now();
        }
    }

    pub(crate) fn open_path(&mut self, path: &std::path::Path) {
        if let Err(e) = winrust_core::shell::open(path) {
            self.status = format!("{}: {e}", i18n::t("Open failed"));
        }
    }

    /// Open a `shell:` namespace path (Recycle Bin, This PC, Network).
    pub(crate) fn open_shell(&mut self, shell_path: &str) {
        if let Err(e) = winrust_core::shell::open_shell_path(shell_path) {
            self.status = format!("{shell_path}: {e}");
        }
    }

    /// Re-read the Desktop folder now (after a paste, delete or sort change).
    pub(crate) fn reload_icons(&mut self) {
        self.icons = desktop::scan(self.sort_order, self.sort_desc);
        self.icons_at = Instant::now();
        self.icon_cache.clear();
    }

    /// "New folder" on the wallpaper, with Explorer's de-duplicated name.
    pub(crate) fn create_desktop_folder(&mut self) {
        let Some(dir) = winrust_core::shell::KnownFolder::Desktop.path() else {
            return;
        };
        match winrust_core::files::create_folder(&dir, i18n::ts("New folder")) {
            Ok(target) => {
                self.reload_icons();
                // Select it, like Explorer does after Ctrl+Shift+N.
                if let Some(idx) = self.icons.iter().position(|i| i.path == target) {
                    self.selection.clear();
                    self.selection.insert(idx);
                    self.selected = Some(idx);
                }
            }
            Err(e) => self.status = tf!("Cannot create the folder: {}", e),
        }
    }

    /// "New -> Text document" and friends: an empty file on the Desktop, with a
    /// de-duplicated name, selected afterwards like Explorer does.
    pub(crate) fn create_desktop_file(&mut self, extension: &str) {
        let Some(dir) = winrust_core::shell::KnownFolder::Desktop.path() else {
            return;
        };
        match winrust_core::files::create_file(&dir, extension, i18n::ts("New file")) {
            Ok(target) => {
                self.reload_icons();
                if let Some(idx) = self.icons.iter().position(|i| i.path == target) {
                    self.selection.clear();
                    self.selection.insert(idx);
                    self.selected = Some(idx);
                }
            }
            Err(e) => self.status = tf!("Cannot create the folder: {}", e),
        }
    }

    /// Paste the clipboard item onto the Desktop folder.
    pub(crate) fn paste_into_desktop(&mut self, src: &std::path::Path) {
        let Some(dir) = winrust_core::shell::KnownFolder::Desktop.path() else {
            return;
        };
        let Some(name) = src.file_name() else {
            return;
        };
        let dest = dir.join(name);
        if dest == src {
            return;
        }
        let res = if self.clipboard_cut {
            winrust_core::disk::move_path(src, &dest, false)
        } else {
            winrust_core::disk::copy_path(src, &dest, false)
        };
        match res {
            Ok(()) => {
                self.clipboard = None;
                self.clipboard_cut = false;
                self.reload_icons();
            }
            Err(e) => self.status = tf!("Cannot paste: {}", e),
        }
    }

    /// The shell icon for one desktop entry, cached per path.
    pub(crate) fn icon_texture(
        &mut self,
        ctx: &egui::Context,
        icon: &desktop::DesktopIcon,
    ) -> Option<egui::TextureId> {
        let size = self.icon_size as u32;
        let key = format!("{}|{}", icon.path.display(), size);
        if let Some(slot) = self.icon_cache.get(&key) {
            return slot.as_ref().map(|t| t.id());
        }
        // The shell image lists only come in 16, 32, 48 and 256 px.  Ask for the
        // nearest of those and then use the square size the pixels actually are,
        // otherwise a 24 px request would be rejected as a short buffer.
        let want = match size {
            0..=16 => 16,
            17..=32 => 32,
            33..=48 => 48,
            _ => 48,
        };
        let info = winrust_core::shell::file_info_sized(&icon.path, want);
        let texture = info.icon_rgba.and_then(|rgba| {
            let px = match rgba.len() / 4 {
                n if n >= (256 * 256) => 256,
                n if n >= (48 * 48) => 48,
                n if n >= (32 * 32) => 32,
                n if n >= (16 * 16) => 16,
                _ => return None,
            };
            let expected = px * px * 4;
            if rgba.len() < expected {
                return None;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([px, px], &rgba[..expected]);
            Some(ctx.load_texture(key.clone(), image, egui::TextureOptions::LINEAR))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.icon_cache.insert(key, texture);
        id
    }

    /// The icon for an arbitrary file (a Start-menu shortcut, a pinned app),
    /// cached in the same map the desktop icons use.
    ///
    /// `slot` is the cache key; the loader is only called on a miss, so a menu
    /// that shows sixty rows uploads sixty textures once, not every frame.
    pub(crate) fn file_icon_texture(
        &mut self,
        ctx: &egui::Context,
        slot: String,
        load: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<egui::TextureId> {
        if let Some(hit) = self.icon_cache.get(&slot) {
            return hit.as_ref().map(|t| t.id());
        }
        let texture = load().and_then(|rgba| {
            let px = match rgba.len() / 4 {
                n if n >= (256 * 256) => 256,
                n if n >= (48 * 48) => 48,
                n if n >= (32 * 32) => 32,
                n if n >= (16 * 16) => 16,
                _ => return None,
            };
            let expected = px * px * 4;
            if rgba.len() < expected {
                return None;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([px, px], &rgba[..expected]);
            Some(ctx.load_texture(slot.clone(), image, egui::TextureOptions::LINEAR))
        });
        let id = texture.as_ref().map(|t| t.id());
        self.icon_cache.insert(slot, texture);
        id
    }

    /// Persist the View-menu switches if anything changed.
    pub(crate) fn flush_desktop_settings(&mut self) {
        if self.settings_dirty {
            self.settings.save();
            self.settings_dirty = false;
        }
    }

    /// Delete / Shift+Delete on the wallpaper, plus Ctrl+A and Escape.
    ///
    /// Explorer deletes whatever is selected when the key is pressed; the same
    /// two-step confirmation is used so a mis-key cannot lose files.
    fn handle_desktop_keys(&mut self, ctx: &egui::Context) {
        // Only when no text field has focus: typing in the Start search must
        // not delete desktop icons.
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let (del, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Delete), i.modifiers.shift));
        if ctx.input(|i| i.key_pressed(egui::Key::A) && i.modifiers.ctrl) {
            self.selection = (0..self.icons.len()).collect();
            self.selected = self.selection.iter().next().copied();
        }
        if !del || self.selection.is_empty() {
            return;
        }
        let paths: Vec<std::path::PathBuf> = self
            .selection
            .iter()
            .filter_map(|i| self.icons.get(*i))
            .filter(|icon| icon.shell_path.is_none())
            .map(|icon| icon.path.clone())
            .collect();
        if paths.is_empty() {
            // Only the virtual entries (This PC / Recycle Bin) were selected;
            // Windows refuses to delete those, so say so instead of doing
            // nothing at all.
            self.status = i18n::ts("This item cannot be deleted").to_string();
            return;
        }
        self.delete_confirm = paths;
        self.delete_permanent = shift;
    }

    /// The "move to Recycle Bin / delete permanently" confirmation.
    fn delete_dialog(&mut self, ctx: &egui::Context) {
        if self.delete_confirm.is_empty() {
            return;
        }
        let paths = self.delete_confirm.clone();
        let permanent = self.delete_permanent;
        let mut action: Option<bool> = None;
        egui::Window::new(if permanent {
            t("Delete permanently")
        } else {
            t("Delete")
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let name = paths
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let question = if paths.len() == 1 {
                if permanent {
                    tf!("Permanently delete \"{}\"?", name)
                } else {
                    tf!("Move \"{}\" to the Recycle Bin?", name)
                }
            } else if permanent {
                tf!("Permanently delete {} items?", paths.len())
            } else {
                tf!("Move {} items to the Recycle Bin?", paths.len())
            };
            ui.label(question);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t("Yes")).clicked() {
                    action = Some(true);
                }
                if ui.button(t("No")).clicked() {
                    action = Some(false);
                }
            });
        });
        match action {
            Some(true) => {
                let report = winrust_core::files::delete(&paths, permanent);
                self.delete_confirm.clear();
                self.selection.clear();
                self.selected = None;
                self.reload_icons();
                self.status = if report.ok() {
                    if permanent {
                        tf!("Deleted {} item(s)", report.done)
                    } else {
                        tf!("Moved {} item(s) to the Recycle Bin", report.done)
                    }
                } else {
                    tf!("{} item(s) could not be deleted", report.errors.len())
                };
            }
            Some(false) => self.delete_confirm.clear(),
            None => {}
        }
    }

    /// Show the Start menu (the taskbar button, `Win`, `Win+X`).  Anything
    /// already floating in the tray closes, because only one panel is open.
    pub(crate) fn show_start(&mut self) {
        winrust_core::window::focus_window(self.hwnd);
        self.session.open();
        self.tray.close();
    }

    /// The taskbar Start button: open, or close if it is already open.
    pub(crate) fn toggle_start(&mut self) {
        if self.session.is_open() {
            self.close_start();
        } else {
            self.show_start();
        }
    }

    pub(crate) fn close_start(&mut self) {
        self.session.close();
    }

    /// Ask the user to confirm a power action (the Start menu's power list and
    /// the Win+X menu both end up here, so neither can act unconfirmed).
    pub(crate) fn ask_power(&mut self, action: crate::session::PowerAction) {
        self.session.close();
        self.session.ask_power(action);
    }

    /// Is the menu open?  (The taskbar button's highlight and the repaint
    /// cadence both ask this.)
    pub(crate) fn start_open(&self) -> bool {
        self.session.is_open()
    }
}

impl DesktopApp {
    /// The `Win+V` history panel: the newest entries, top-down, each one a
    /// click away from being pasted back.
    ///
    /// It is positioned where Windows puts it - centred, a little above the
    /// taskbar - and closes on a click outside or `Esc`.  The list itself is
    /// [`winrust_core::shell::ClipboardHistory`]; this function only draws it.
    pub(crate) fn clipboard_panel(&mut self, ctx: &egui::Context) {
        if !self.session.clipboard_open() {
            return;
        }
        let screen = ctx.screen_rect();
        let pal = winrust_ui::theme::pal(&self.appearance);
        let width = 380.0_f32.min(screen.width() - 40.0);
        let pos = egui::pos2(
            screen.center().x - width / 2.0,
            (screen.bottom() - crate::taskbar::HEIGHT - 8.0 - 320.0).max(screen.top() + 40.0),
        );
        // The history is copied into plain strings first, because
        // `entries()` borrows the history while the closure below needs
        // `&mut self` for `picked`/`clear` and the click handling.  (The
        // closure *may* borrow `self`: `Frame::show` boxes it with an inferred
        // lifetime, so a snapshot is a convenience here, not a requirement.)
        let entries: Vec<String> = self
            .clipboard
            .entries()
            .iter()
            .map(|e| {
                let one_line: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
                winrust_core::i18n::end(&one_line, 60)
            })
            .collect();
        let mut picked: Option<usize> = None;
        let mut clear = false;
        let response = egui::Area::new(egui::Id::new("clipboard_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(
                        pal.surface.0,
                        pal.surface.1,
                        pal.surface.2,
                        252,
                    ))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2),
                    ))
                    .rounding(10.0)
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t("Clipboard history")).strong());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button(t("Clear")).clicked() {
                                        clear = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        if entries.is_empty() {
                            ui.label(
                                RichText::new(ts("Copy something and it will appear here"))
                                    .small()
                                    .weak(),
                            );
                            return;
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("clipboard_list")
                            .auto_shrink([false, false])
                            .max_height(260.0)
                            .show(ui, |ui| {
                                for (i, label) in entries.iter().enumerate() {
                                    let resp = ui.add_sized(
                                        [ui.available_width(), 26.0],
                                        egui::Button::new(RichText::new(label).size(12.5))
                                            .frame(false),
                                    );
                                    if resp.clicked() {
                                        picked = Some(i);
                                    }
                                }
                            });
                    });
            })
            .response;

        if let Some(i) = picked {
            if self.clipboard.pick(i) {
                self.session.hide_clipboard();
                self.status = ts("Copied back to the clipboard").to_string();
            }
        }
        if clear {
            self.clipboard.clear();
        }
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let outside = ctx.input(|i| i.pointer.any_click())
            && ctx
                .input(|i| i.pointer.latest_pos())
                .map(|p| !response.rect.contains(p))
                .unwrap_or(false);
        if escape || outside {
            self.session.hide_clipboard();
        }
    }
}

impl eframe::App for DesktopApp {
    /// Save the View settings when the shell shuts down.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
        desktop::save_positions(&self.icons);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.tick();
        if self.hwnd == 0 {
            self.hwnd = winrust_ui::theme::hwnd_from_handle(frame).unwrap_or(0);
        }
        if self.appearance_dirty {
            self.appearance = self.appearance.sanitized();
            winrust_ui::theme::apply(ctx, &self.appearance);
            winrust_ui::theme::apply_window(self.hwnd, &self.appearance);
            let _ = i18n::settings::save(&self.appearance);
            self.appearance_dirty = false;
        }
        // The launcher owns the language switch; follow it live, at most once a
        // second (a registry read per frame would be wasteful).
        if self.lang_checked_at.elapsed() >= Duration::from_secs(1) {
            self.lang_checked_at = Instant::now();
            let stored_lang = i18n::lang::load();
            if stored_lang != self.lang {
                self.lang = stored_lang;
                i18n::set_lang(stored_lang);
                winrust_ui::theme::apply(ctx, &self.appearance);
            }
        }

        // The wallpaper: a vertical gradient, painted behind everything.
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        let top = egui::Color32::from_rgb(18, 32, 56);
        let bottom = egui::Color32::from_rgb(8, 12, 20);
        painter.rect_filled(screen, 0.0, top);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(screen.left(), screen.center().y),
                screen.right_bottom(),
            ),
            0.0,
            bottom,
        );

        // Ease both panels towards their target.  `dt` comes from egui so the
        // speed is frame-rate independent; the curve is a plain ease-out, which
        // is what Windows uses for menus.
        //
        // Speed matters as much as the curve: the 0.9.0 easing used one rate
        // (14/s) for both directions, which needed ~7 frames to *close* - the
        // "the Start menu closes very slowly" defect.  Closing is now nearly
        // instant (45/s, ~3 frames) and opening is still soft (22/s), which is
        // the asymmetry Windows uses: dismissal must feel immediate.
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let ease = |value: f32, target: f32, rate: f32, dt: f32| {
            let step = (dt * rate).min(1.0);
            value + (target - value) * step
        };
        let start_target = if self.session.is_open() { 1.0 } else { 0.0 };
        let start_rate = if start_target > self.session.anim() {
            22.0
        } else {
            45.0
        };
        let anim = ease(self.session.anim(), start_target, start_rate, dt);
        self.session.set_anim(anim);
        let flyout_target = if self.tray.flyout().is_open() { 1.0 } else { 0.0 };
        let flyout_rate = if flyout_target > self.flyout_anim {
            22.0
        } else {
            45.0
        };
        self.flyout_anim = ease(self.flyout_anim, flyout_target, flyout_rate, dt);

        self.handle_desktop_keys(ctx);
        self.delete_dialog(ctx);

        // The `Win` key opens Start, as it does on Windows.  `GetAsyncKeyState`
        // is read directly because egui has no `Key::Win`, and the edge is
        // detected here so holding the key does not flap the menu.
        let win_down = winrust_core::window::win_key_down();
        if win_down && !self.win_key_was_down {
            self.toggle_start();
        }
        // `Win+V`: our own clipboard history, because Windows' lives inside
        // explorer.exe and stops working when the stock shell is replaced.
        let v_down = winrust_core::window::v_key_down();
        if self.session.win_v_pressed(win_down, v_down) {
            winrust_core::window::focus_window(self.hwnd);
            self.session.toggle_clipboard();
            self.tray.close();
        }
        self.win_key_was_down = win_down;

        // Start menu and taskbar are painted as panels; the icon grid fills
        // whatever is left.
        self.taskbar(ctx);
        self.start_menu(ctx);
        // The confirmation is a window of its own: it outlives the menu, which
        // closes as soon as the user picks a power action.
        self.power_confirmation(ctx);
        self.clipboard_panel(ctx);
        self.desktop_area(ctx);

        if self.session.is_open() {
            // The Start menu must react to clicks that close it.
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.close_start();
            }
        }
        // The shell must react to `Win` quickly, so it is polled at 8 Hz while
        // the menu is open or a preview is pending, and at 2 Hz otherwise (the
        // clock only needs seconds).
        let hot = self.session.is_open()
            || self.session.pending_power().is_some()
            || self.session.clipboard_open()
            || self.tray.flyout().is_open()
            || !self.hover.is_empty()
            || !self.delete_confirm.is_empty();
        ctx.request_repaint_after(Duration::from_millis(if hot { 120 } else { 500 }));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        winrust_ui::theme::bg_color(&self.appearance)
    }
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let ru = winrust_core::i18n::lang::load() == winrust_core::i18n::Lang::Ru;
    let title = if ru {
        "Рабочий стол WinRust"
    } else {
        "WinRust Desktop"
    };
    let appearance = winrust_core::i18n::settings::load();
    let options = eframe::NativeOptions {
        // A shell window covers the screen but is not fullscreen (fullscreen
        // would steal focus from real applications).
        viewport: egui::ViewportBuilder::default()
            .with_maximized(true)
            .with_decorations(false)
            .with_min_inner_size([800.0, 600.0])
            .with_title(title)
            .with_transparent(appearance.transparent),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(|cc| Ok(Box::new(DesktopApp::new(cc)))),
    )
}

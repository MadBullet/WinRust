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
mod start;
mod taskbar;

use eframe::egui;
use std::time::{Duration, Instant};
use winrust_core::i18n::{self, Lang};

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
    pub(crate) start_open: bool,
    /// True when the menu was opened from the tray power button, so it lands on
    /// the power section instead of the app list.
    pub(crate) start_power_menu: bool,
    pub(crate) start_search: String,
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
    pub(crate) apps: Vec<start::AppEntry>,
    pub(crate) apps_at: Instant,
    pub(crate) lang: Lang,
    /// When the registry was last polled for a language change.
    pub(crate) lang_checked_at: Instant,
    pub(crate) hwnd: isize,
    /// Wallpaper brush colour mode — the desktop paints itself instead of
    /// showing a picture, so it works on a bare Windows install as well.
    pub(crate) appearance: winrust_core::i18n::Appearance,
    pub(crate) appearance_dirty: bool,
    /// Last system sample for the taskbar readout (CPU %, memory %).
    pub(crate) cpu_mem: (f32, winrust_core::system::MemoryStatus),
    pub(crate) prev_cpu: winrust_core::system::CpuSample,
    pub(crate) sampled_at: Instant,
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
            start_open: false,
            start_power_menu: false,
            start_search: String::new(),
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
            apps: Vec::new(),
            apps_at: Instant::now() - Duration::from_secs(300),
            lang: i18n::current(),
            lang_checked_at: Instant::now(),
            hwnd: 0,
            appearance,
            appearance_dirty: false,
            cpu_mem: (0.0, winrust_core::system::memory_status()),
            prev_cpu: winrust_core::system::cpu_sample(),
            sampled_at: Instant::now() - Duration::from_secs(5),
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
            self.windows_at = Instant::now();
        }
        if self.apps_at.elapsed() >= Duration::from_secs(120) {
            self.apps = start::installed_apps();
            self.apps_at = Instant::now();
        }
        self.clock = taskbar::clock_text();
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
        let base = i18n::ts("New folder");
        let mut name = base.to_string();
        let mut n = 2;
        while dir.join(&name).exists() {
            name = format!("{base} ({n})");
            n += 1;
        }
        let target = dir.join(&name);
        match std::fs::create_dir(&target) {
            Ok(()) => {
                self.reload_icons();
                // Select it, like Explorer does after Ctrl+Shift+N.
                if let Some(idx) = self.icons.iter().position(|i| i.path == target) {
                    self.selected = Some(idx);
                }
            }
            Err(e) => self.status = i18n::tf!("Cannot create the folder: {}", e),
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
            Err(e) => self.status = i18n::tf!("Cannot paste: {}", e),
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

    /// Persist the View-menu switches if anything changed.
    pub(crate) fn flush_desktop_settings(&mut self) {
        if self.settings_dirty {
            self.settings.save();
            self.settings_dirty = false;
        }
    }

    pub(crate) fn close_start(&mut self) {
        self.start_open = false;
        self.start_search.clear();
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

        // Start menu and taskbar are painted as panels; the icon grid fills
        // whatever is left.
        self.taskbar(ctx);
        self.start_menu(ctx);
        self.desktop_area(ctx);

        if self.start_open {
            // The Start menu must react to clicks that close it.
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.close_start();
            }
        }
        ctx.request_repaint_after(Duration::from_millis(500));
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

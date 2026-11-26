//! Task Manager — processes, performance, startup, users, network, services.
//!
//! Layout
//! ------
//! The stock Windows Task Manager keeps its CPU / Memory / Disk / Network / GPU
//! columns in a wide block in the middle.  Here the same six metrics live in a
//! **vertical icon strip on the left edge**, so the process name never competes
//! with the numbers and the window works on narrow screens.  Captions can be
//! shown or hidden (the ↔ button) and the strip remembers the choice.
//!
//! Sampler model
//! -------------
//! One background thread refreshes processes, I/O counters, GPU and system CPU
//! twice per second and hands the UI a finished snapshot; the UI thread never
//! blocks.  Process identity is `(pid, creation time)` so recycled PIDs are not
//! confused when rows move between frames.
//!
//! Robustness
//! ----------
//! Release builds use `panic = "abort"`, so *any* panic (even in a background
//! thread) terminates the process without a message.  The previous version had
//! a byte-slicing `shorten_path` that panicked on Cyrillic paths, which is
//! exactly what a Russian Windows supplies (`C:\Users\Иван\...`).  Every string
//! truncation now goes through the Unicode-safe helpers in `winrust_core`, and
//! the sampler is wrapped so a failing Win32 call yields empty data instead of
//! unwinding.

mod format;
mod model;
mod sort;
mod startup;
mod tabs;
mod ui;

pub use sort::{RowFilter, SortBy, UpdateInterval};
pub use tabs::Tab;

use eframe::egui;
use egui::RichText;
use std::time::{Duration, Instant};
use winrust_core::i18n::{self, Appearance, Lang};
use winrust_core::tf;

use winrust_core::gpu;
use winrust_core::net::{self, Connection};
use winrust_core::service::ScManager;
use winrust_core::system;
use winrust_core::user;

use model::{spawn_sampler, Sampler, Snapshot};
use startup::StartupItem;

pub struct TaskManagerApp {
    pub(super) tab: Tab,
    pub(super) sampler: Sampler,
    pub(super) snapshot: Snapshot,
    pub(super) history_cpu: Vec<f32>,
    pub(super) history_mem: Vec<f32>,
    pub(super) history_gpu: Vec<f32>,
    pub(super) history_net: Vec<f32>,
    pub(super) selected: Option<u32>,
    pub(super) user: user::UserInfo,
    pub(super) os: String,
    pub(super) cpu_info: system::CpuInfo,
    pub(super) ram_installed: Option<u64>,
    pub(super) gpus: Vec<(u32, String)>,
    pub(super) search: String,
    pub(super) group_by_app: bool,
    /// Groups whose arrow is pointing right: their rows are hidden.  Windows
    /// calls them "Приложения" and "Фоновые процессы" and both collapse.
    pub(super) apps_collapsed: bool,
    pub(super) background_collapsed: bool,
    pub(super) row_filter: RowFilter,
    pub(super) sort_by: SortBy,
    pub(super) sort_desc: bool,
    /// How often the list redraws from the sampler.  Windows calls this the
    /// "update speed" and sets it from a right-click on the list.
    pub(super) update_interval: UpdateInterval,
    /// When the visible snapshot was last taken from the sampler, so the
    /// chosen interval can hold back newer samples.
    pub(super) shown_at: Instant,
    /// The newest sample the sampler has produced; `snapshot` is the one the
    /// interval allowed through.
    pub(super) pending_snapshot: Option<Snapshot>,
    pub(super) status: String,
    pub(super) confirm_kill: Option<(u32, String)>,
    pub(super) startup: Vec<StartupItem>,
    pub(super) startup_loaded: bool,
    pub(super) connections: Vec<Connection>,
    pub(super) last_conn_refresh: Instant,
    pub(super) services: Vec<winrust_core::service::ServiceEntry>,
    pub(super) last_service_refresh: Instant,
    pub(super) priority_open: Option<u32>,
    /// Left metric strip: show the text captions next to the icons.
    pub(super) strip_labels: bool,
    /// Opacity/blur of this window, shared with the other components.
    pub(super) appearance: Appearance,
    pub(super) lang: Lang,
    /// When the registry was last polled for a language change.
    pub(super) lang_checked_at: std::time::Instant,
    /// Window handle captured on the first frame, for the DWM calls.
    pub(super) hwnd: isize,
    pub(super) appearance_dirty: bool,
}

impl TaskManagerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        i18n::init();
        let appearance = i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);
        let sampler = spawn_sampler(Duration::from_millis(500));
        let cpu_info = system::cpu_info();
        Self {
            tab: Tab::Processes,
            sampler,
            snapshot: Snapshot::default(),
            history_cpu: vec![0.0; 120],
            history_mem: vec![0.0; 120],
            history_gpu: vec![0.0; 120],
            history_net: vec![0.0; 120],
            selected: None,
            user: user::current_user(),
            os: system::os_summary(),
            cpu_info,
            ram_installed: system::installed_ram_bytes(),
            gpus: gpu::adapters(),
            search: String::new(),
            group_by_app: true,
            apps_collapsed: false,
            background_collapsed: false,
            row_filter: RowFilter::All,
            sort_by: SortBy::Cpu,
            sort_desc: true,
            update_interval: UpdateInterval::OneSecond,
            shown_at: Instant::now(),
            pending_snapshot: None,
            status: String::new(),
            confirm_kill: None,
            startup: Vec::new(),
            startup_loaded: false,
            connections: Vec::new(),
            last_conn_refresh: Instant::now() - Duration::from_secs(10),
            services: Vec::new(),
            last_service_refresh: Instant::now() - Duration::from_secs(10),
            priority_open: None,
            strip_labels: true,
            appearance,
            lang: i18n::current(),
            lang_checked_at: std::time::Instant::now(),
            hwnd: 0,
            appearance_dirty: false,
        }
    }

    pub(super) fn pump(&mut self, ctx: &egui::Context) {
        // The sampler runs at its own fixed rate; the *view* decides how often
        // it adopts a sample.  Keeping the newest one aside and showing it when
        // the chosen interval has elapsed is what makes "update speed" a
        // setting instead of a re-spawn of the worker thread.
        let mut newest: Option<Snapshot> = None;
        while let Ok(snap) = self.sampler.rx.try_recv() {
            newest = Some(snap);
        }
        if let Some(snap) = newest {
            self.pending_snapshot = Some(snap);
        }
        if self.shown_at.elapsed() < self.update_interval.hold() {
            // Not time yet: keep the samples coming so the next tick shows the
            // freshest one, and repaint so the UI stays responsive.
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
            self.tick_side_panels();
            return;
        }
        let Some(snap) = self.pending_snapshot.take() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            self.tick_side_panels();
            return;
        };
        self.shown_at = Instant::now();
        {
            // `remove(0)` panics on an empty vector; the histories are created
            // with 120 zeros, but hardening costs nothing and the abort-on-panic
            // profile makes every panic fatal.
            if self.history_cpu.len() >= 120 {
                self.history_cpu.remove(0);
            }
            self.history_cpu.push(snap.cpu_percent);
            if self.history_mem.len() >= 120 {
                self.history_mem.remove(0);
            }
            self.history_mem.push(snap.memory.used_percent());
            if self.history_gpu.len() >= 120 {
                self.history_gpu.remove(0);
            }
            self.history_gpu.push(snap.gpu.best());
            let net_total: f64 = snap.rows.iter().map(|r| r.net_bps).sum();
            if self.history_net.len() >= 120 {
                self.history_net.remove(0);
            }
            self.history_net.push(net_total as f32);
            self.snapshot = snap;
        }
        self.tick_side_panels();
    }

    /// The slow polls that are independent of the process list's update speed:
    /// connections and, on the Services tab, the service list.
    fn tick_side_panels(&mut self) {
        if self.last_conn_refresh.elapsed().as_secs() >= 2 {
            self.connections = net::tcp_connections();
            self.last_conn_refresh = Instant::now();
        }
        if self.last_service_refresh.elapsed().as_secs() >= 5 && self.tab == Tab::Services {
            if let Ok(scm) = ScManager::connect() {
                if let Ok(list) = scm.enumerate() {
                    self.services = list;
                }
            }
            self.last_service_refresh = Instant::now();
        }
        if self.tab == Tab::Startup && !self.startup_loaded {
            self.startup = StartupItem::scan();
            self.startup_loaded = true;
        }
    }

    /// Re-apply transparency/blur once per change, never every frame.
    pub(super) fn apply_appearance(&mut self, ctx: &egui::Context) {
        // The launcher owns the language switch; follow it live, at most once a
        // second (a registry read per frame would be wasteful).
        if self.lang_checked_at.elapsed() >= std::time::Duration::from_secs(1) {
            self.lang_checked_at = std::time::Instant::now();
            let stored_lang = i18n::lang::load();
            if stored_lang != self.lang {
                self.lang = stored_lang;
                i18n::set_lang(stored_lang);
                winrust_ui::theme::apply(ctx, &self.appearance);
            }
        }
        if self.appearance_dirty {
            self.appearance = self.appearance.sanitized();
            winrust_ui::theme::apply(ctx, &self.appearance);
            winrust_ui::theme::apply_window(self.hwnd, &self.appearance);
            let _ = i18n::settings::save(&self.appearance);
            self.appearance_dirty = false;
        }
    }

    pub(super) fn set_lang(&mut self, lang: Lang, ctx: &egui::Context) {
        self.lang = lang;
        i18n::set_lang(lang);
        let _ = i18n::lang::save(lang);
        // Font setup depends on which Cyrillic-capable font was found, so the
        // visuals are rebuilt from scratch.
        winrust_ui::theme::apply(ctx, &self.appearance);
    }
}

impl eframe::App for TaskManagerApp {
    /// The window's clear colour: the theme background with the configured
    /// opacity, so transparency reaches the compositor in every program.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        winrust_ui::theme::bg_color(&self.appearance)
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.pump(ctx);
        if self.hwnd == 0 {
            self.hwnd = winrust_ui::theme::hwnd_from_handle(frame).unwrap_or(0);
            if self.appearance.transparent {
                self.appearance_dirty = true;
            }
        }
        self.apply_appearance(ctx);

        // Left icon strip: the six metrics, then the tab buttons.
        egui::SidePanel::left("metric_strip")
            .exact_width(if self.strip_labels { 96.0 } else { 52.0 })
            .resizable(false)
            .show(ctx, |ui| self.metric_strip(ui));

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.header(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tf!(
                        "{} processes · CPU {}% · Memory {}% · refresh {} ms",
                        self.snapshot.rows.len(),
                        format_args!("{:.1}", self.snapshot.cpu_percent),
                        format_args!("{:.1}", self.snapshot.memory.used_percent()),
                        self.snapshot.took.as_millis()
                    ))
                    .small()
                    .weak(),
                );
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Processes => self.processes_tab(ui),
            Tab::Details => self.details_tab(ui),
            Tab::Performance => self.performance_tab(ui),
            Tab::Startup => self.startup_tab(ui),
            Tab::Users => self.users_tab(ui),
            Tab::Network => self.network_tab(ui),
            Tab::Services => self.services_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });
        self.priority_window(ctx);
        self.confirm_modal(ctx);
    }
}

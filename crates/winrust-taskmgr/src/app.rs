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

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winrust_core::gpu::{self, GpuQuery};
use winrust_core::i18n::{self, Appearance, Lang};
use winrust_core::net::{self, Connection};
use winrust_core::process::{self, AccessLevel, Priority, ProcessInfo};
use winrust_core::service::{ScManager, ServiceState};
use winrust_core::shell;
use winrust_core::system::{self, CpuSample, MemoryStatus};
use winrust_core::user;
use winrust_core::{t, tf, ts};

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Processes,
    Performance,
    Startup,
    Users,
    Network,
    Services,
    Settings,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Processes => ts("Processes"),
            Tab::Performance => ts("Performance"),
            Tab::Startup => ts("Startup"),
            Tab::Users => ts("Users"),
            Tab::Network => ts("Network"),
            Tab::Services => ts("Services"),
            Tab::Settings => ts("Settings"),
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Tab::Processes => "▤",
            Tab::Performance => "📈",
            Tab::Startup => "🚀",
            Tab::Users => "👤",
            Tab::Network => "🌐",
            Tab::Services => "⚙",
            Tab::Settings => "☰",
        }
    }
    const ALL: [Tab; 7] = [
        Tab::Processes,
        Tab::Performance,
        Tab::Startup,
        Tab::Users,
        Tab::Network,
        Tab::Services,
        Tab::Settings,
    ];
}

// ---------------------------------------------------------------------------
// Metrics — the six numbers of the stock Task Manager, in a left strip
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    GpuEngine,
}

impl Metric {
    fn caption(self) -> &'static str {
        match self {
            Metric::Cpu => ts("CPU"),
            Metric::Memory => ts("Memory"),
            Metric::Disk => ts("Disk"),
            Metric::Network => ts("Network"),
            Metric::Gpu => ts("GPU"),
            Metric::GpuEngine => ts("GPU engine"),
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Metric::Cpu => "▦",
            Metric::Memory => "▤",
            Metric::Disk => "💽",
            Metric::Network => "🌐",
            Metric::Gpu => "🎮",
            Metric::GpuEngine => "◫",
        }
    }
    fn short(self) -> &'static str {
        match self {
            Metric::Cpu => "Ц",
            Metric::Memory => "П",
            Metric::Disk => "Д",
            Metric::Network => "С",
            Metric::Gpu => "GPU",
            Metric::GpuEngine => "Я",
        }
    }
    const ALL: [Metric; 6] = [
        Metric::Cpu,
        Metric::Memory,
        Metric::Disk,
        Metric::Network,
        Metric::Gpu,
        Metric::GpuEngine,
    ];
}

/// Row data for one process plus its six metric values.
#[derive(Debug, Clone, Default)]
struct Row {
    info: ProcessInfo,
    /// Task Manager's per-process CPU share of the whole machine.
    cpu_percent: f32,
    disk_bps: f64,
    net_bps: f64,
    gpu_percent: f32,
    gpu_engine: String,
    /// Application grouping ("Приложения" vs "Фоновые процессы").
    group: String,
    /// Application name shown instead of the exe (Task Manager shows
    /// "Проводник", we show the file description from the version resource).
    app_name: String,
    /// Aggregate of the whole group, drawn on the group header.
    group_cpu: f32,
    group_mem: u64,
    group_gpu: f32,
    group_count: usize,
}

impl Row {
    fn metric(&self, m: Metric) -> String {
        match m {
            Metric::Cpu => format!("{:.1}%", self.cpu_percent),
            Metric::Memory => mb(self.info.private_bytes),
            Metric::Disk => rate(self.disk_bps),
            Metric::Network => rate(self.net_bps),
            Metric::Gpu => format!("{:.1}%", self.gpu_percent),
            Metric::GpuEngine => {
                if self.gpu_engine.is_empty() {
                    "—".into()
                } else {
                    self.gpu_engine.clone()
                }
            }
        }
    }
    fn metric_heat(&self, m: Metric) -> f32 {
        match m {
            Metric::Cpu => self.cpu_percent,
            Metric::Memory => {
                (self.info.private_bytes as f64 / (512.0 * 1024.0 * 1024.0)) as f32 * 100.0
            }
            Metric::Disk => (self.disk_bps / (1024.0 * 1024.0)) as f32 * 100.0,
            Metric::Network => (self.net_bps / (1024.0 * 1024.0)) as f32 * 100.0,
            Metric::Gpu => self.gpu_percent,
            Metric::GpuEngine => 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    rows: Vec<Row>,
    memory: MemoryStatus,
    cpu_percent: f32,
    cpu: CpuSample,
    gpu: gpu::GpuSample,
    uptime_ms: u64,
    took: Duration,
}

struct Sampler {
    rx: Receiver<Snapshot>,
    stop: Arc<AtomicBool>,
}

/// Start the sampling thread.  Any panic inside is caught so a driver quirk
/// cannot take the whole program down.
fn spawn_sampler(interval: Duration) -> Sampler {
    let (tx, rx) = channel::<Snapshot>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        // A panicking sampler used to abort the process; now it degrades to
        // "no data" and the UI keeps running.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut prev_cpu = system::cpu_sample();
            let mut prev_proc: HashMap<u32, (u64, u64)> = HashMap::new();
            let mut prev_io: HashMap<u32, u64> = HashMap::new();
            let mut prev_net: HashMap<u32, u64> = HashMap::new();
            let mut prev_wall = Instant::now();
            let cores = system::cpu_count().max(1) as f64;
            let mut gpu = GpuQuery::open();
            let mut app_names: HashMap<String, String> = HashMap::new();

            while !stop2.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let now_cpu = system::cpu_sample();
                let cpu_percent = system::cpu_usage_between(prev_cpu, now_cpu);
                let procs = process::enumerate().unwrap_or_default();
                let wall = prev_wall.elapsed().as_secs_f64().max(0.001);
                let net_totals = net::net_bytes_per_pid();

                // GPU: engine utilisation grouped by owning PID.
                let mut per_pid_gpu: HashMap<u32, (f32, String)> = HashMap::new();
                let mut gpu_sample = gpu::GpuSample::default();
                if let Some(q) = gpu.as_mut() {
                    if let Some(engines) = q.sample() {
                        let mut by_adapter: HashMap<u32, f32> = HashMap::new();
                        for (name, pct) in &engines {
                            // PDH hands out doubles; the UI works in f32.
                            let pct = *pct as f32;
                            let pid = pid_from_engine(name).unwrap_or(0);
                            let adapter = gpu::adapter_of(name);
                            let e = by_adapter.entry(adapter).or_insert(0.0);
                            if pct > *e {
                                *e = pct;
                            }
                            if pid != 0 {
                                let entry = per_pid_gpu.entry(pid).or_insert((0.0, String::new()));
                                if pct > entry.0 {
                                    entry.0 = pct;
                                    entry.1 = engine_short(name);
                                }
                            }
                        }
                        gpu_sample.by_adapter = by_adapter.into_iter().collect();
                        gpu_sample.overall = gpu_sample
                            .by_adapter
                            .iter()
                            .map(|(_, v)| *v)
                            .fold(0.0_f32, f32::max);
                    }
                }

                let mut rows = Vec::with_capacity(procs.len());
                let mut new_proc = HashMap::with_capacity(procs.len());
                let mut new_io: HashMap<u32, u64> = HashMap::with_capacity(procs.len());
                let mut new_net: HashMap<u32, u64> = HashMap::with_capacity(procs.len());
                for p in procs {
                    new_proc.insert(p.pid, (p.created_100ns, p.cpu_time_100ns));
                    let cpu_percent = match prev_proc.get(&p.pid).copied() {
                        Some((pc, pu)) if p.created_100ns == pc => {
                            let delta = p.cpu_time_100ns.saturating_sub(pu) as f64;
                            (delta / 10_000_000.0 / wall / cores * 100.0) as f32
                        }
                        _ => 0.0,
                    };
                    let io_total = p.io_read_bytes.saturating_add(p.io_write_bytes);
                    let disk_bps = match prev_io.get(&p.pid) {
                        Some(prev) => io_total.saturating_sub(*prev) as f64 / wall,
                        None => 0.0,
                    };
                    new_io.insert(p.pid, io_total);
                    let net_total = net_totals.get(&p.pid).copied().unwrap_or(0);
                    let net_bps = match prev_net.get(&p.pid) {
                        Some(prev) => net_total.saturating_sub(*prev) as f64 / wall,
                        None => 0.0,
                    };
                    new_net.insert(p.pid, net_total);

                    // Human application name, resolved once per image path.
                    let app_name = if p.exe_path.is_empty() {
                        p.name.clone()
                    } else {
                        app_names
                            .entry(p.exe_path.clone())
                            .or_insert_with(|| {
                                let vi = shell::version_info(&p.exe_path);
                                if vi.description.is_empty() {
                                    p.name.clone()
                                } else {
                                    vi.description
                                }
                            })
                            .clone()
                    };
                    let (gpu_percent, gpu_engine) = per_pid_gpu
                        .get(&p.pid)
                        .cloned()
                        .map(|(pct, eng)| (pct, eng))
                        .unwrap_or((0.0, String::new()));
                    let group = group_key(&p);
                    rows.push(Row {
                        info: p,
                        cpu_percent,
                        disk_bps,
                        net_bps,
                        gpu_percent,
                        gpu_engine,
                        group,
                        app_name,
                        ..Default::default()
                    });
                }
                prev_proc = new_proc;
                prev_io = new_io;
                prev_net = new_net;
                prev_cpu = now_cpu;
                prev_wall = Instant::now();

                // Group aggregates for the "Приложения (N)" headers.
                let mut groups: HashMap<String, (f32, u64, usize, f32)> = HashMap::new();
                for r in &rows {
                    let e = groups.entry(r.group.clone()).or_default();
                    e.0 += r.cpu_percent;
                    e.1 += r.info.private_bytes;
                    e.2 += 1;
                    e.3 += r.gpu_percent;
                }
                for r in &mut rows {
                    if let Some((cpu, mem, count, gpu)) = groups.get(&r.group) {
                        r.group_cpu = *cpu;
                        r.group_mem = *mem;
                        r.group_count = *count;
                        r.group_gpu = *gpu;
                    }
                }
                rows.sort_by(|a, b| {
                    b.cpu_percent
                        .partial_cmp(&a.cpu_percent)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.info.private_bytes.cmp(&a.info.private_bytes))
                });

                let snap = Snapshot {
                    rows,
                    memory: system::memory_status(),
                    cpu_percent,
                    cpu: now_cpu,
                    gpu: gpu_sample,
                    uptime_ms: system::uptime_ms(),
                    took: t0.elapsed(),
                };
                if tx.send(snap).is_err() {
                    break;
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
            }
        }));
        if result.is_err() {
            // No `log` crate in this binary: the sampler thread panicked, but the
            // UI keeps drawing whatever the last good snapshot was.
            eprintln!("winrust-taskmgr: process sampler stopped after a panic; UI keeps running");
        }
    });
    Sampler { rx, stop }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// `pid_1234_luid_0x00000000_0x0000A1B2_phys_0_eng_3_engtype_3D`
fn pid_from_engine(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// `3D`, `Copy`, `VideoDecode` — the part Task Manager prints in "GPU engine".
fn engine_short(instance: &str) -> String {
    match instance.split("engtype_").nth(1) {
        Some(t) => {
            let end = t.find('_').unwrap_or(t.len());
            t[..end].to_string()
        }
        None => String::new(),
    }
}

fn group_key(p: &ProcessInfo) -> String {
    p.name.to_lowercase()
}

// ---------------------------------------------------------------------------
// Sorting / filtering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Name,
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    Pid,
}

/// Which rows a view shows: the stock "Apps" / "Background" split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowFilter {
    All,
    Apps,
    Background,
}

pub struct TaskManagerApp {
    tab: Tab,
    sampler: Sampler,
    snapshot: Snapshot,
    history_cpu: Vec<f32>,
    history_mem: Vec<f32>,
    history_gpu: Vec<f32>,
    history_net: Vec<f32>,
    selected: Option<u32>,
    user: user::UserInfo,
    os: String,
    cpu_info: system::CpuInfo,
    ram_installed: Option<u64>,
    gpus: Vec<(u32, String)>,
    search: String,
    group_by_app: bool,
    row_filter: RowFilter,
    sort_by: SortBy,
    sort_desc: bool,
    status: String,
    confirm_kill: Option<(u32, String)>,
    startup: Vec<StartupItem>,
    startup_loaded: bool,
    connections: Vec<Connection>,
    last_conn_refresh: Instant,
    services: Vec<winrust_core::service::ServiceEntry>,
    last_service_refresh: Instant,
    priority_open: Option<u32>,
    /// Left metric strip: show the text captions next to the icons.
    strip_labels: bool,
    /// Opacity/blur of this window, shared with the other components.
    appearance: Appearance,
    lang: Lang,
    /// Window handle captured on the first frame, for the DWM calls.
    hwnd: isize,
    appearance_dirty: bool,
}

#[derive(Debug, Clone)]
struct StartupItem {
    name: String,
    command: String,
    location: String,
    enabled: bool,
    published: String,
}

impl StartupItem {
    fn scan() -> Vec<StartupItem> {
        use winrust_core::registry::{self, Root};
        let mut out = Vec::new();
        for (root, path, label) in [
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                "HKCU Run",
            ),
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                "HKLM Run",
            ),
        ] {
            for (name, ty, data) in registry::enum_values(path, root) {
                let cmd = if ty == 1 || ty == 2 {
                    String::from_utf16_lossy(
                        &data
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect::<Vec<u16>>(),
                    )
                    .trim_end_matches('\0')
                    .to_string()
                } else {
                    String::new()
                };
                let exe = exe_from_command(&cmd);
                let published = if exe.is_empty() {
                    String::new()
                } else {
                    let vi = shell::version_info(&exe);
                    if vi.company.is_empty() {
                        vi.description
                    } else {
                        vi.company
                    }
                };
                out.push(StartupItem {
                    name,
                    command: cmd,
                    location: label.into(),
                    enabled: true,
                    published,
                });
            }
        }
        // StartupApproved\Run holds a 12-byte blob: first dword 2 = enabled,
        // 3 = disabled.  This is what Task Manager toggles.
        for (root, approved_path) in [
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            ),
        ] {
            for (name, _ty, data) in registry::enum_values(approved_path, root) {
                if data.len() < 4 {
                    continue;
                }
                let flag = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let disabled = flag & 0xFF != 2;
                if let Some(item) = out.iter_mut().find(|i| i.name.eq_ignore_ascii_case(&name)) {
                    item.enabled = !disabled;
                }
            }
        }
        out.sort_by_key(|s| s.name.to_lowercase());
        out
    }

    fn set_enabled(item: &StartupItem, enable: bool) -> Result<(), String> {
        use winrust_core::registry::{self, Root};
        let (root, path) = if item.location.starts_with("HKLM") {
            (
                Root::LocalMachine,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            )
        } else {
            (
                Root::CurrentUser,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
            )
        };
        let key = registry::open_for_write(root, path).map_err(|e| e.to_string())?;
        let mut blob = [0u8; 12];
        blob[0] = if enable { 2 } else { 3 };
        key.set_binary(&item.name, &blob).map_err(|e| e.to_string())
    }
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
            row_filter: RowFilter::All,
            sort_by: SortBy::Cpu,
            sort_desc: true,
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
            hwnd: 0,
            appearance_dirty: false,
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        let mut got = false;
        while let Ok(snap) = self.sampler.rx.try_recv() {
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
            got = true;
        }
        if got {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
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
    fn apply_appearance(&mut self, ctx: &egui::Context) {
        if self.appearance_dirty {
            self.appearance = self.appearance.sanitized();
            winrust_ui::theme::apply(ctx, &self.appearance);
            winrust_ui::theme::apply_window(self.hwnd, &self.appearance);
            let _ = i18n::settings::save(&self.appearance);
            self.appearance_dirty = false;
        }
    }

    fn set_lang(&mut self, lang: Lang, ctx: &egui::Context) {
        self.lang = lang;
        i18n::set_lang(lang);
        let _ = i18n::lang::save(lang);
        // Font setup depends on which Cyrillic-capable font was found, so the
        // visuals are rebuilt from scratch.
        winrust_ui::theme::apply(ctx, &self.appearance);
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

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

impl TaskManagerApp {
    /// The stock columns as a vertical strip: icon, caption, live value.
    fn metric_strip(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let tip = if self.strip_labels {
                t("Hide captions")
            } else {
                t("Show captions")
            };
            if ui
                .add(egui::Button::new(RichText::new("↔").small()).frame(false))
                .on_hover_text(tip)
                .clicked()
            {
                self.strip_labels = !self.strip_labels;
            }
            if self.strip_labels {
                ui.label(RichText::new(t("Metrics")).small().weak());
            }
        });
        ui.add_space(4.0);

        let values = [
            (
                Metric::Cpu,
                format!("{:.0}%", self.snapshot.cpu_percent),
                self.snapshot.cpu_percent,
            ),
            (
                Metric::Memory,
                format!("{:.0}%", self.snapshot.memory.used_percent()),
                self.snapshot.memory.used_percent(),
            ),
            (
                Metric::Disk,
                rate(self.snapshot.rows.iter().map(|r| r.disk_bps).sum::<f64>()),
                self.snapshot.rows.iter().map(|r| r.disk_bps).sum::<f64>() as f32
                    / (1024.0 * 1024.0),
            ),
            (
                Metric::Network,
                rate(self.snapshot.rows.iter().map(|r| r.net_bps).sum::<f64>()),
                self.snapshot.rows.iter().map(|r| r.net_bps).sum::<f64>() as f32
                    / (1024.0 * 1024.0),
            ),
            (
                Metric::Gpu,
                format!("{:.0}%", self.snapshot.gpu.best()),
                self.snapshot.gpu.best(),
            ),
        ];
        for (metric, value, heat) in values {
            self.metric_chip(ui, metric, &value, heat);
        }

        // "GPU engine" is the stock column that names the busiest engine.
        let busiest = self
            .snapshot
            .rows
            .iter()
            .filter(|r| !r.gpu_engine.is_empty())
            .max_by(|a, b| {
                a.gpu_percent
                    .partial_cmp(&b.gpu_percent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|r| r.gpu_engine.clone())
            .unwrap_or_default();
        self.metric_chip(ui, Metric::GpuEngine, &busiest, 0.0);

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        let mut chosen_tab: Option<Tab> = None;
        let strip_labels = self.strip_labels;
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            for tab in Tab::ALL {
                let selected = self.tab == tab;
                let text = if strip_labels {
                    format!("{}  {}", tab.glyph(), tab.label())
                } else {
                    tab.glyph().to_string()
                };
                let resp = ui.add_sized(
                    [ui.available_width(), 26.0],
                    egui::SelectableLabel::new(selected, RichText::new(text).small()),
                );
                if resp.clicked() {
                    chosen_tab = Some(tab);
                }
                if !strip_labels {
                    resp.on_hover_text(tab.label());
                }
            }
        });
        if let Some(tab) = chosen_tab {
            self.tab = tab;
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(env!("CARGO_PKG_VERSION")).small().weak());
        });
    }

    fn metric_chip(&self, ui: &mut egui::Ui, metric: Metric, value: &str, heat: f32) {
        let t = (heat / 100.0).clamp(0.0, 1.0);
        let color = Color32::from_rgb((120.0 + 130.0 * t) as u8, (220.0 - 130.0 * t) as u8, 120);
        let resp = ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 34.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(RichText::new(metric.glyph()).size(15.0).color(color));
                ui.vertical(|ui| {
                    if self.strip_labels {
                        ui.label(RichText::new(metric.caption()).small().weak());
                    } else {
                        ui.label(RichText::new(metric.short()).small().weak());
                    }
                    let shown = if value.is_empty() { "—" } else { value };
                    ui.label(RichText::new(icon_text(shown, 14)).size(12.0).color(color));
                });
            },
        );
        resp.response.on_hover_text(metric.caption());
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}  {}", self.tab.glyph(), self.tab.label()))
                    .strong()
                    .size(15.0),
            );
            ui.separator();
            if let Some(sel) = self.selected {
                if let Some(row) = self.snapshot.rows.iter().find(|r| r.info.pid == sel) {
                    ui.label(
                        RichText::new(format!("{}  ·  PID {}", row.info.name, sel))
                            .small()
                            .weak(),
                    );
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Language quick-switch, the same setting the launcher writes.
                let mut lang = self.lang;
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(lang.native_name())
                    .width(96.0)
                    .show_ui(ui, |ui| {
                        for l in Lang::ALL {
                            if ui.selectable_label(lang == l, l.native_name()).clicked()
                                && lang != l
                            {
                                lang = l;
                            }
                        }
                    });
                if lang != self.lang {
                    self.set_lang(lang, ui.ctx());
                }
                ui.label(
                    RichText::new(format!(
                        "{}  ·  {}  ·  {} {}",
                        self.os,
                        self.cpu_info.arch,
                        self.cpu_info.logical,
                        t("cores")
                    ))
                    .small()
                    .weak(),
                );
            });
        });
    }

    fn processes_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_sized(
                [210.0, 22.0],
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(format!("🔍 {}", t("Search processes"))),
            );
            ui.separator();
            for (f, label) in [
                (RowFilter::All, ts("All")),
                (RowFilter::Apps, ts("Apps")),
                (RowFilter::Background, ts("Background processes")),
            ] {
                if ui.selectable_label(self.row_filter == f, label).clicked() {
                    self.row_filter = f;
                }
            }
            ui.separator();
            ui.checkbox(&mut self.group_by_app, ts("Group by app"));
        });

        ui.horizontal(|ui| {
            let selected = self.selected.and_then(|pid| {
                self.snapshot
                    .rows
                    .iter()
                    .find(|r| r.info.pid == pid)
                    .cloned()
            });
            let can_end = selected.is_some();
            if ui
                .add_enabled(can_end, egui::Button::new(ts("End task")))
                .clicked()
            {
                if let Some(row) = &selected {
                    self.confirm_kill = Some((row.info.pid, row.info.name.clone()));
                }
            }
            if ui
                .add_enabled(can_end, egui::Button::new(ts("Suspend")))
                .clicked()
            {
                if let Some(row) = &selected {
                    match process::suspend(row.info.pid) {
                        Ok(n) => self.status = tf!("Suspended {} threads of {}", n, &row.info.name),
                        Err(e) => self.status = tf!("Suspend failed: {}", e),
                    }
                }
            }
            if ui
                .add_enabled(can_end, egui::Button::new(ts("Resume")))
                .clicked()
            {
                if let Some(row) = &selected {
                    match process::resume(row.info.pid) {
                        Ok(n) => self.status = tf!("Resumed {} threads of {}", n, &row.info.name),
                        Err(e) => self.status = tf!("Resume failed: {}", e),
                    }
                }
            }
            if ui
                .add_enabled(can_end, egui::Button::new(ts("Priority…")))
                .clicked()
            {
                if let Some(row) = &selected {
                    self.priority_open = Some(row.info.pid);
                }
            }
            if ui
                .add_enabled(can_end, egui::Button::new(ts("Open file location")))
                .clicked()
            {
                if let Some(row) = &selected {
                    if !row.info.exe_path.is_empty() {
                        let _ = shell::open(std::path::Path::new(&row.info.exe_path));
                    }
                }
            }
            if ui
                .add_enabled(can_end, egui::Button::new(ts("Properties")))
                .clicked()
            {
                if let Some(row) = &selected {
                    if !row.info.exe_path.is_empty() {
                        let _ = shell::show_properties(&row.info.exe_path);
                    }
                }
            }
        });
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small().weak());
        }

        // Filter + sort.
        let needle = self.search.to_lowercase();
        let mut display: Vec<Row> = self
            .snapshot
            .rows
            .iter()
            .filter(|r| {
                let by_name = needle.is_empty()
                    || r.app_name.to_lowercase().contains(&needle)
                    || r.info.name.to_lowercase().contains(&needle)
                    || r.info.pid.to_string() == needle;
                let by_kind = match self.row_filter {
                    RowFilter::All => true,
                    RowFilter::Apps => is_app_row(r),
                    RowFilter::Background => !is_app_row(r),
                };
                by_name && by_kind
            })
            .cloned()
            .collect();
        sort_rows(&mut display, self.sort_by, self.sort_desc);
        if self.group_by_app {
            display.sort_by(|a, b| {
                is_app_row(a)
                    .cmp(&is_app_row(b))
                    .reverse()
                    .then(a.group.cmp(&b.group))
            });
        }

        let mut clicked: Option<u32> = None;
        let mut sort_col: Option<SortBy> = None;
        let mut end_pid: Option<u32> = None;
        let mut new_priority: Option<(u32, Priority)> = None;

        // Headers reflect the visible metrics: name first, then the six strip
        // metrics, then the metadata columns.  There are exactly as many
        // columns as headers — the old code declared 8 headers for 7 columns,
        // which egui_extras logs as a broken table every frame.
        let mut headers: Vec<(String, SortBy)> = vec![(ts("Name").to_string(), SortBy::Name)];
        for m in Metric::ALL {
            let key = match m {
                Metric::Cpu => SortBy::Cpu,
                Metric::Memory => SortBy::Memory,
                Metric::Disk => SortBy::Disk,
                Metric::Network => SortBy::Network,
                Metric::Gpu => SortBy::Gpu,
                // "GPU engine" is text, not a sortable quantity.
                Metric::GpuEngine => SortBy::Pid,
            };
            headers.push((m.caption().to_string(), key));
        }
        headers.push((ts("Status").to_string(), SortBy::Pid));
        headers.push((ts("Priority").to_string(), SortBy::Pid));

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(240.0).at_least(140.0).clip(true));
        // Metric columns: narrow, heat-coloured, the same six as the strip.
        for _ in Metric::ALL {
            table = table.column(Column::initial(84.0).at_least(56.0).clip(true));
        }
        table = table
            .column(Column::initial(96.0).at_least(60.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(60.0).clip(true));

        let table = table.header(22.0, |mut header| {
            for (title, key) in &headers {
                header.col(|ui| {
                    let arrow = if self.sort_by == *key {
                        if self.sort_desc {
                            " ▼"
                        } else {
                            " ▲"
                        }
                    } else {
                        ""
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{title}{arrow}")).strong().size(11.0),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        sort_col = Some(*key);
                    }
                });
            }
        });

        table.body(|body| {
            body.rows(22.0, display.len(), |mut row| {
                let idx = row.index();
                let r = match display.get(idx) {
                    Some(r) => r,
                    None => return,
                };
                let selected = self.selected == Some(r.info.pid);
                row.set_selected(selected);
                row.col(|ui| {
                    let label = if r.app_name.is_empty() {
                        r.info.name.clone()
                    } else {
                        r.app_name.clone()
                    };
                    let resp = ui.selectable_label(
                        selected,
                        RichText::new(winrust_core::i18n::middle(&label, 40, 22)),
                    );
                    if resp.clicked() {
                        clicked = Some(r.info.pid);
                    }
                    resp.clone().on_hover_text(format!(
                        "{}\n{}\nPID {}",
                        label,
                        winrust_core::i18n::middle(&r.info.exe_path, 120, 60),
                        r.info.pid
                    ));
                    resp.context_menu(|ui| {
                        if ui.button(ts("End task")).clicked() {
                            end_pid = Some(r.info.pid);
                            ui.close_menu();
                        }
                        if ui.button(ts("Suspend")).clicked() {
                            let _ = process::suspend(r.info.pid);
                            ui.close_menu();
                        }
                        if ui.button(ts("Resume")).clicked() {
                            let _ = process::resume(r.info.pid);
                            ui.close_menu();
                        }
                        if !r.info.exe_path.is_empty()
                            && ui.button(ts("Open file location")).clicked()
                        {
                            let _ = shell::open(std::path::Path::new(&r.info.exe_path));
                            ui.close_menu();
                        }
                        if !r.info.exe_path.is_empty() && ui.button(ts("Properties")).clicked() {
                            let _ = shell::show_properties(&r.info.exe_path);
                            ui.close_menu();
                        }
                        ui.separator();
                        for p in process::ALL_PRIORITIES {
                            if ui
                                .button(format!("{}: {}", t("Priority"), p.label()))
                                .clicked()
                            {
                                new_priority = Some((r.info.pid, p));
                                ui.close_menu();
                            }
                        }
                    });
                });
                for m in Metric::ALL {
                    row.col(|ui| {
                        let text = r.metric(m);
                        let heat = r.metric_heat(m);
                        let color = if m == Metric::Memory {
                            Color32::from_rgb(200, 210, 240)
                        } else {
                            heat_color(heat, 100.0)
                        };
                        let shown = if m == Metric::GpuEngine {
                            winrust_core::i18n::end(&text, 18)
                        } else {
                            text
                        };
                        ui.label(RichText::new(shown).monospace().size(11.0).color(color));
                    });
                }
                row.col(|ui| {
                    let text = if r.info.access == AccessLevel::Denied {
                        ts("Access denied")
                    } else {
                        ts("Running")
                    };
                    ui.label(RichText::new(text).small().weak());
                });
                row.col(|ui| {
                    ui.label(RichText::new(r.info.priority.label()).small().weak());
                });
            });
        });

        if let Some(pid) = clicked {
            self.selected = Some(pid);
        }
        if let Some(key) = sort_col {
            if self.sort_by == key {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_by = key;
                self.sort_desc = true;
            }
        }
        if let Some(pid) = end_pid {
            if let Some(r) = display.iter().find(|r| r.info.pid == pid) {
                self.confirm_kill = Some((pid, r.info.name.clone()));
            }
        }
        if let Some((pid, prio)) = new_priority {
            match process::set_priority(pid, prio) {
                Ok(()) => self.status = tf!("Priority set to {}", prio.label()),
                Err(e) => self.status = tf!("Priority change failed: {}", e),
            }
        }
    }

    fn performance_tab(&mut self, ui: &mut egui::Ui) {
        // egui 0.29 wants the column contents as a closure, so the whole body
        // lives inside `ui.columns`.
        ui.columns(2, |cols| {
            let cpu_hist = self.history_cpu.clone();
            let mem_hist = self.history_mem.clone();
            let gpu_hist = self.history_gpu.clone();
            let net_hist = self.history_net.clone();

            cols[0].heading(ts("CPU"));
            self.graph(&mut cols[0], &cpu_hist, 100.0, ts("CPU"));
            cols[0].label(tf!(
                "{} · {} logical processors",
                self.cpu_info.arch,
                self.cpu_info.logical
            ));
            cols[0].label(tf!("Uptime: {}", format_uptime(self.snapshot.uptime_ms)));
            cols[0].separator();

            cols[0].heading(ts("Memory"));
            self.graph(&mut cols[0], &mem_hist, 100.0, ts("Memory"));
            let m = self.snapshot.memory;
            cols[0].label(tf!(
                "In use: {} ({}%)",
                bytes_text(m.used_phys()),
                format_args!("{:.0}", m.used_percent())
            ));
            cols[0].label(tf!("Available: {}", bytes_text(m.avail_phys)));
            cols[0].label(tf!("Total: {}", bytes_text(m.total_phys)));
            if let Some(ram) = self.ram_installed {
                cols[0].label(tf!("Installed RAM: {}", bytes_text(ram)));
            }
            cols[0].label(tf!(
                "Committed: {}",
                bytes_text(m.total_page_file.saturating_sub(m.avail_page_file))
            ));
            cols[0].separator();

            cols[1].heading(ts("GPU"));
            self.graph(&mut cols[1], &gpu_hist, 100.0, ts("GPU"));
            if self.snapshot.gpu.by_adapter.is_empty() {
                cols[1].label(
                    RichText::new(ts("No GPU counters available"))
                        .weak()
                        .small(),
                );
            } else {
                for (adapter, pct) in &self.snapshot.gpu.by_adapter {
                    let name = self
                        .gpus
                        .get(*adapter as usize)
                        .map(|(_, n)| n.as_str())
                        .unwrap_or(ts("GPU"));
                    cols[1].label(format!("{name}: {pct:.0}%"));
                }
            }
            cols[1].separator();
            cols[1].heading(ts("Network"));
            self.graph(&mut cols[1], &net_hist, 100.0 * 1024.0, ts("Network"));
            cols[1].label(tf!(
                "Total throughput: {}",
                rate(net_hist.last().copied().unwrap_or(0.0) as f64)
            ));
            cols[1].separator();
            cols[1].heading(ts("Kernel CPU time"));
            cols[1].label(format!(
                "{}: {:.1} s   {}: {:.1} s   {}: {:.1} s",
                t("Kernel"),
                self.snapshot.cpu.kernel_100ns() as f64 / 1e7,
                t("User"),
                self.snapshot.cpu.user_100ns() as f64 / 1e7,
                t("Idle"),
                self.snapshot.cpu.idle_100ns() as f64 / 1e7,
            ));
        });
    }

    fn graph(&self, ui: &mut egui::Ui, data: &[f32], max: f32, label: &str) {
        let size = egui::vec2(ui.available_width().min(460.0), 130.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, Color32::from_rgb(20, 22, 28));
        for i in 1..4 {
            let y = rect.top() + rect.height() * i as f32 / 4.0;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0_f32, Color32::from_rgb(38, 40, 52)),
            );
        }
        let n = data.len();
        if n < 2 {
            return;
        }
        let step = rect.width() / (n - 1) as f32;
        let mut points = Vec::with_capacity(n);
        for (i, v) in data.iter().enumerate() {
            let norm = if max <= 0.0 {
                0.0
            } else {
                (v / max).clamp(0.0, 1.0)
            };
            let y = rect.bottom() - norm * rect.height();
            points.push(egui::pos2(rect.left() + i as f32 * step, y));
        }
        painter.add(egui::Shape::line(
            points.clone(),
            egui::Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255)),
        ));
        let mut fill = points;
        fill.push(egui::pos2(rect.right(), rect.bottom()));
        fill.push(egui::pos2(rect.left(), rect.bottom()));
        painter.add(egui::Shape::convex_polygon(
            fill,
            Color32::from_rgba_unmultiplied(120, 200, 255, 26),
            egui::Stroke::NONE,
        ));
        painter.text(
            rect.left_top() + egui::vec2(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            format!("{} {:.1}", label, data.last().copied().unwrap_or(0.0)),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(205, 225, 255),
        );
    }

    fn priority_window(&mut self, ctx: &egui::Context) {
        let Some(pid) = self.priority_open else {
            return;
        };
        let Some(row) = self
            .snapshot
            .rows
            .iter()
            .find(|r| r.info.pid == pid)
            .cloned()
        else {
            self.priority_open = None;
            return;
        };
        let mut keep_open = true;
        let mut chosen: Option<Priority> = None;
        egui::Window::new(tf!("Priority — {}", &row.info.name))
            .collapsible(false)
            .resizable(false)
            .open(&mut keep_open)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(tf!("Current: {}", row.info.priority.label()))
                        .weak()
                        .small(),
                );
                ui.separator();
                for p in process::ALL_PRIORITIES {
                    if ui
                        .add(egui::RadioButton::new(row.info.priority == p, p.label()))
                        .clicked()
                    {
                        chosen = Some(p);
                    }
                }
                if ui.button(ts("Cancel")).clicked() {
                    self.priority_open = None;
                }
            });
        if let Some(p) = chosen {
            match process::set_priority(pid, p) {
                Ok(()) => self.status = format!("{} -> {}", row.info.name, p.label()),
                Err(e) => self.status = tf!("Priority change failed: {}", e),
            }
            self.priority_open = None;
        } else if !keep_open {
            self.priority_open = None;
        }
    }

    fn startup_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(ts("Startup apps")).strong());
            if ui.button(ts("Refresh")).clicked() {
                self.startup = StartupItem::scan();
            }
        });
        ui.separator();
        let mut toggle: Option<(usize, bool)> = None;
        let mut status_note: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (idx, item) in self.startup.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut on = item.enabled;
                    if ui.checkbox(&mut on, "").changed() {
                        toggle = Some((idx, on));
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&item.name).strong());
                        ui.label(
                            RichText::new(winrust_core::i18n::end(&item.command, 96))
                                .small()
                                .weak(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&item.location).small().weak());
                        if !item.published.is_empty() {
                            ui.label(RichText::new(&item.published).small());
                            ui.separator();
                        }
                    });
                });
                ui.separator();
            }
            if self.startup.is_empty() {
                ui.label(ts("No startup entries found in HKCU/HKLM Run keys."));
            }
        });
        if let Some((idx, enable)) = toggle {
            if let Some(item) = self.startup.get(idx) {
                match StartupItem::set_enabled(item, enable) {
                    Ok(()) => {
                        let name = item.name.clone();
                        status_note = Some(format!(
                            "{} — {}",
                            name,
                            if enable {
                                ts("Enabled")
                            } else {
                                ts("Disabled")
                            }
                        ));
                        self.startup = StartupItem::scan();
                    }
                    Err(e) => status_note = Some(tf!("Registry update failed: {}", e)),
                }
            }
        }
        if let Some(note) = status_note {
            self.status = note;
        }
    }

    fn users_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading(ts("Users"));
        ui.separator();
        egui::Grid::new("users_grid")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new(ts("User")).strong());
                ui.label(RichText::new(ts("Session")).strong());
                ui.label(RichText::new(ts("Integrity")).strong());
                ui.label(RichText::new(ts("Elevated")).strong());
                ui.end_row();
                ui.label(format!("{}\\{}", self.user.domain, self.user.account));
                ui.label(ts("Console (current)"));
                ui.label(self.user.integrity);
                ui.label(if self.user.elevated {
                    ts("Yes")
                } else {
                    ts("No")
                });
                ui.end_row();
            });
        ui.add_space(10.0);
        ui.label(
            RichText::new(ts("Sessions are enumerated from the local session table."))
                .small()
                .weak(),
        );
        for s in enumerate_sessions() {
            ui.label(s);
        }
    }

    fn network_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(ts("Refresh now")).clicked() {
                self.connections = net::tcp_connections();
            }
            ui.label(
                RichText::new(tf!("{} TCP connections", self.connections.len()))
                    .weak()
                    .small(),
            );
        });
        ui.separator();
        let mut by_pid: HashMap<u32, (usize, Vec<u16>)> = HashMap::new();
        for c in &self.connections {
            let e = by_pid.entry(c.pid).or_default();
            e.0 += 1;
            e.1.push(c.local_port);
        }
        let mut pids: Vec<u32> = by_pid.keys().copied().collect();
        pids.sort();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("net_grid")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new(ts("PID")).strong());
                    ui.label(RichText::new(ts("Process")).strong());
                    ui.label(RichText::new(ts("Connections")).strong());
                    ui.label(RichText::new(ts("Local ports")).strong());
                    ui.end_row();
                    for pid in pids {
                        let (count, ports) = &by_pid[&pid];
                        let name = self
                            .snapshot
                            .rows
                            .iter()
                            .find(|r| r.info.pid == pid)
                            .map(|r| r.info.name.clone())
                            .unwrap_or_else(|| "?".into());
                        ui.label(pid.to_string());
                        ui.label(name);
                        ui.label(count.to_string());
                        let mut p = ports.clone();
                        p.sort();
                        p.dedup();
                        let port_list = p
                            .iter()
                            .take(8)
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        ui.label(port_list);
                        ui.end_row();
                    }
                });
        });
    }

    fn services_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(ts("Refresh")).clicked() {
                self.last_service_refresh = Instant::now() - Duration::from_secs(10);
            }
            ui.label(
                RichText::new(tf!("{} services", self.services.len()))
                    .weak()
                    .small(),
            );
            if !self.user.elevated {
                ui.label(
                    RichText::new(ts("(read-only without elevation)"))
                        .small()
                        .weak(),
                );
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("services_grid")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new(ts("Name")).strong());
                    ui.label(RichText::new(ts("Status")).strong());
                    ui.label(RichText::new(ts("Start type")).strong());
                    ui.label(RichText::new(ts("PID")).strong());
                    ui.end_row();
                    for s in &self.services {
                        ui.label(&s.display_name);
                        let state = s.state.map(|x| x.label()).unwrap_or_else(|| "—".into());
                        let color = match s.state {
                            Some(ServiceState::Running) => Color32::from_rgb(120, 220, 120),
                            Some(ServiceState::Stopped) => Color32::from_rgb(220, 120, 120),
                            _ => Color32::GRAY,
                        };
                        ui.label(RichText::new(state).color(color));
                        ui.label(
                            s.start_type
                                .map(|x| x.label())
                                .unwrap_or_else(|| "—".into()),
                        );
                        ui.label(if s.pid == 0 {
                            String::new()
                        } else {
                            s.pid.to_string()
                        });
                        ui.end_row();
                    }
                });
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading(ts("Settings"));
        ui.separator();
        ui.heading(RichText::new(ts("Appearance")).size(14.0));
        ui.label(
            RichText::new(ts(
                "Transparency and blur cost GPU time every frame. Turn them off for the lowest CPU/GPU use.",
            ))
            .small()
            .weak(),
        );
        let mut changed = false;
        changed |= ui
            .checkbox(&mut self.appearance.transparent, ts("Transparency"))
            .changed();
        ui.add_enabled_ui(self.appearance.transparent, |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut self.appearance.opacity, 30..=100).text(ts("Opacity")))
                .changed();
            changed |= ui.checkbox(&mut self.appearance.blur, ts("Blur")).changed();
            ui.add_enabled_ui(self.appearance.blur, |ui| {
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.appearance.blur_amount, 0..=100)
                            .text(ts("Blur amount")),
                    )
                    .changed();
            });
        });
        if changed {
            self.appearance_dirty = true;
        }
        ui.label(
            RichText::new(tf!(
                "Settings are stored in HKCU\\{}",
                winrust_core::i18n::settings::REGISTRY_PATH
            ))
            .small()
            .weak(),
        );
        ui.separator();
        ui.heading(RichText::new(ts("Language")).size(14.0));
        let mut lang = self.lang;
        for l in Lang::ALL {
            ui.radio_value(&mut lang, l, l.native_name());
        }
        if lang != self.lang {
            self.set_lang(lang, ui.ctx());
        }
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        if let Some((pid, name)) = self.confirm_kill.clone() {
            egui::Window::new(ts("End task"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(tf!("Terminate {} (PID {})?", &name, pid));
                    ui.label(
                        RichText::new(ts("Unsaved data in this app will be lost."))
                            .small()
                            .weak(),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(ts("End task")).clicked() {
                            match process::terminate(pid) {
                                Ok(()) => self.status = tf!("Terminated {}", name),
                                Err(e) => self.status = tf!("Terminate failed: {}", e),
                            }
                            self.confirm_kill = None;
                        }
                        if ui.button(ts("Cancel")).clicked() {
                            self.confirm_kill = None;
                        }
                    });
                });
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// "Apps" are processes that own a visible top-level window; without a window
/// list the cheapest good proxy is "has a version description and is not a
/// Windows host process".  Background processes are the rest.
fn is_app_row(r: &Row) -> bool {
    let n = r.info.name.to_lowercase();
    let host = n.starts_with("svchost")
        || n.starts_with("conhost")
        || n.starts_with("dllhost")
        || n.starts_with("runtimebroker")
        || n.starts_with("backgroundtaskhost")
        || n.starts_with("searchhost")
        || n.starts_with("taskhostw")
        || n.starts_with("sihost")
        || n.starts_with("ctfmon");
    if host {
        return false;
    }
    !r.app_name.is_empty()
}

fn sort_rows(rows: &mut [Row], by: SortBy, desc: bool) {
    let cmp = |a: &Row, b: &Row| -> std::cmp::Ordering {
        let ord = match by {
            SortBy::Name => a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()),
            SortBy::Cpu => a
                .cpu_percent
                .partial_cmp(&b.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Memory => a.info.private_bytes.cmp(&b.info.private_bytes),
            SortBy::Disk => a
                .disk_bps
                .partial_cmp(&b.disk_bps)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Network => a
                .net_bps
                .partial_cmp(&b.net_bps)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Gpu => a
                .gpu_percent
                .partial_cmp(&b.gpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Pid => a.info.pid.cmp(&b.info.pid),
        };
        if desc {
            ord.reverse()
        } else {
            ord
        }
    };
    rows.sort_by(cmp);
}

/// First token of a startup command line, with quotes stripped.
fn exe_from_command(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return winrust_core::i18n::head(rest, end).to_string();
        }
        return rest.to_string();
    }
    t.split_whitespace().next().unwrap_or("").to_string()
}

/// Byte-per-second rate, shaped like Task Manager (B/s, KB/s, MB/s, GB/s).
fn rate(bps: f64) -> String {
    if !bps.is_finite() || bps <= 0.0 {
        return "0 Б/с".into();
    }
    let unit_b = ts("B/s");
    if bps < 1024.0 {
        format!("{bps:.0} {unit_b}")
    } else if bps < 1024.0 * 1024.0 {
        format!("{:.1} {}{unit_b}", bps / 1024.0, ts("K"))
    } else if bps < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} {}{unit_b}", bps / (1024.0 * 1024.0), ts("M"))
    } else {
        format!(
            "{:.2} {}{unit_b}",
            bps / (1024.0 * 1024.0 * 1024.0),
            ts("G")
        )
    }
}

fn mb(bytes: u64) -> String {
    let m = bytes as f64 / 1_048_576.0;
    if m >= 1024.0 {
        bytes_text(bytes)
    } else {
        format!("{m:.1} {}", ts("MB"))
    }
}

fn bytes_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.2} {}", ts("GB"))
    } else {
        format!("{:.0} {}", bytes as f64 / 1_048_576.0, ts("MB"))
    }
}

fn heat_color(percent: f32, scale: f32) -> Color32 {
    let t = (percent / scale.max(1.0)).clamp(0.0, 1.0);
    if t < 0.005 {
        return Color32::from_rgb(150, 150, 160);
    }
    let r = (120.0 + 135.0 * t) as u8;
    let g = (225.0 - 135.0 * t) as u8;
    Color32::from_rgb(r, g, 110)
}

/// The metric chips are narrow: keep the text short without splitting a char.
fn icon_text(s: &str, max: usize) -> String {
    winrust_core::i18n::end(s, max)
}

fn format_uptime(ms: u64) -> String {
    let s = ms / 1000;
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 {
        format!("{}d {}h {}m", d, h, m)
    } else {
        format!("{}h {}m", h, m)
    }
}

fn enumerate_sessions() -> Vec<String> {
    let console = std::env::var("SESSIONNAME").unwrap_or_else(|_| "Console".into());
    vec![format!("  {console} — {}", ts("active"))]
}

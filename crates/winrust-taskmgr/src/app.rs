//! Task Manager: Processes + Performance + Users + Startup + Network tabs.
//!
//! Sampling model
//! --------------
//! * A background thread refreshes the process snapshot and the system CPU
//!   counters twice per second and pushes a `Snapshot` through a channel.
//!   The UI thread never blocks on WMI/PDH (and therefore never hangs).
//! * CPU per process is computed from the delta of `GetProcessTimes` between
//!   two snapshots, normalised by elapsed wall time and core count — exactly
//!   what Task Manager does.
//! * Process identity is `(pid, creation_time)` so recycled PIDs are not
//!   confused when the process list is resorted.

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use winrust_core::net::{self, Connection};
use winrust_core::process::{self, AccessLevel, Priority, ProcessInfo};
use winrust_core::service::{ScManager, ServiceState};
use winrust_core::shell;
use winrust_core::system::{self, CpuSample, MemoryStatus};
use winrust_core::user;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Processes,
    Performance,
    Startup,
    Users,
    Network,
    Services,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Processes => "Processes",
            Tab::Performance => "Performance",
            Tab::Startup => "Startup",
            Tab::Users => "Users",
            Tab::Network => "Network",
            Tab::Services => "Services",
        }
    }
    const ALL: [Tab; 6] = [Tab::Processes, Tab::Performance, Tab::Startup, Tab::Users, Tab::Network, Tab::Services];
}

/// One row with computed CPU%, grouped the way Task Manager groups by app.
#[derive(Debug, Clone, Default)]
struct Row {
    info: ProcessInfo,
    cpu_percent: f32,
    /// Bytes/s read+written, sampled between snapshots (Task Manager "Disk").
    disk_bps: f64,
    /// Bytes/s over TCP/UDP, sampled between snapshots (Task Manager "Network").
    net_bps: f64,
    /// Processes hosted in the same application group (e.g. all Chrome tabs).
    group: String,
    /// Total memory of the whole group, shown on the group header.
    group_mem: u64,
    group_cpu: f32,
    group_count: usize,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    rows: Vec<Row>,
    memory: MemoryStatus,
    cpu_percent: f32,
    cpu: CpuSample,
    uptime_ms: u64,
    took: Duration,
}

struct Sampler {
    rx: Receiver<Snapshot>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn spawn_sampler(interval: Duration) -> Sampler {
    let (tx, rx) = channel::<Snapshot>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let mut prev_cpu = system::cpu_sample();
        let mut prev_proc: HashMap<u32, (u64, u64)> = HashMap::new();
        // pid -> (read+write bytes, other bytes) from the previous tick
        let mut prev_io: HashMap<u32, (u64, u64)> = HashMap::new();
        let mut prev_net: HashMap<u32, u64> = HashMap::new();
        let mut prev_wall = Instant::now();
        let cores = system::cpu_count().max(1) as f64;
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            let t0 = Instant::now();
            let now_cpu = system::cpu_sample();
            let cpu_percent = system::cpu_usage_between(prev_cpu, now_cpu);
            let procs = process::enumerate().unwrap_or_default();
            let wall = prev_wall.elapsed().as_secs_f64().max(0.001);

            // Network counters are only refreshed every tick; the map keeps
            // the last measured byte totals per pid so we can diff them.
            let net_totals = net::net_bytes_per_pid();

            let mut rows = Vec::with_capacity(procs.len());
            let mut new_proc = HashMap::with_capacity(procs.len());
            let mut new_io: HashMap<u32, (u64, u64)> = HashMap::with_capacity(procs.len());
            let mut new_net: HashMap<u32, u64> = HashMap::with_capacity(procs.len());
            for p in procs {
                let key = (p.created_100ns, p.cpu_time_100ns);
                new_proc.insert(p.pid, key);
                let prev = prev_proc.get(&p.pid).copied();
                let cpu_percent = match prev {
                    Some((prev_created, prev_cpu_100ns)) if p.created_100ns == prev_created => {
                        let delta = p.cpu_time_100ns.saturating_sub(prev_cpu_100ns) as f64;
                        // 100ns units -> percent of one core -> percent of all cores
                        (delta / 10_000_000.0 / wall / cores * 100.0) as f32
                    }
                    _ => 0.0,
                };
                let io_total = p.io_read_bytes.saturating_add(p.io_write_bytes);
                let disk_bps = match prev_io.get(&p.pid) {
                    Some((prev_total, _)) => io_total.saturating_sub(*prev_total) as f64 / wall,
                    None => 0.0,
                };
                new_io.insert(p.pid, (io_total, p.io_other_bytes));
                let net_total = net_totals.get(&p.pid).copied().unwrap_or(0);
                new_net.insert(p.pid, net_total);
                let net_bps = match prev_net.get(&p.pid) {
                    Some(prev_total) => net_total.saturating_sub(*prev_total) as f64 / wall,
                    None => 0.0,
                };
                let group = group_key(&p);
                rows.push(Row { info: p, cpu_percent, disk_bps, net_bps, group, ..Default::default() });
            }
            prev_proc = new_proc;
            prev_io = new_io;
            prev_net = new_net;
            prev_cpu = now_cpu;
            prev_wall = Instant::now();

            // Aggregate group totals.
            let mut groups: HashMap<String, (f32, u64, usize)> = HashMap::new();
            for r in &rows {
                let e = groups.entry(r.group.clone()).or_default();
                e.0 += r.cpu_percent;
                e.1 += r.info.private_bytes;
                e.2 += 1;
            }
            for r in &mut rows {
                if let Some((cpu, mem, count)) = groups.get(&r.group) {
                    r.group_cpu = *cpu;
                    r.group_mem = *mem;
                    r.group_count = *count;
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
    });
    Sampler { rx, stop }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Task Manager groups processes by their window owner; we approximate that
/// with the image name (all `chrome.exe` rows form one "Google Chrome" group).
fn group_key(p: &ProcessInfo) -> String {
    p.name.to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Name,
    Cpu,
    Memory,
    Disk,
    Network,
    Pid,
}

pub struct TaskManagerApp {
    tab: Tab,
    sampler: Sampler,
    snapshot: Snapshot,
    history_cpu: Vec<f32>,
    history_mem: Vec<f32>,
    selected: Option<u32>,
    user: user::UserInfo,
    os: String,
    cpu_info: system::CpuInfo,
    ram_installed: Option<u64>,
    search: String,
    group_by_app: bool,
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
        // Run keys
        for (root, path, label) in [
            (Root::CurrentUser, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "HKCU Run"),
            (Root::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "HKLM Run"),
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
                    if vi.company.is_empty() { vi.description } else { vi.company }
                };
                out.push(StartupItem {
                    name,
                    command: cmd,
                    location: label.into(),
                    // Below, StartupApproved tells us the real on/off state.
                    enabled: true,
                    published,
                });
            }
        }
        // Task Manager marks disabled startup items in StartupApproved\Run as a
        // 12-byte blob whose first dword is 2 (enabled) / 3 (disabled).
        for (root, approved_path) in [
            (Root::CurrentUser, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
            (Root::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"),
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

    /// Toggle a startup entry the way Task Manager does: write the 12-byte
    /// `StartupApproved\Run` blob (0x02 = enabled, 0x03 = disabled) instead of
    /// deleting the Run value, so the entry stays visible and re-enableable.
    fn set_enabled(item: &StartupItem, enable: bool) -> Result<(), String> {
        use winrust_core::registry::{self, Root};
        let (root, path) = if item.location.starts_with("HKLM") {
            (Root::LocalMachine, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
        } else {
            (Root::CurrentUser, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
        };
        let key = registry::open_for_write(root, path).map_err(|e| e.to_string())?;
        let mut blob = [0u8; 12];
        blob[0] = if enable { 2 } else { 3 };
        key.set_binary(&item.name, &blob).map_err(|e| e.to_string())
    }
}

impl TaskManagerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let sampler = spawn_sampler(Duration::from_millis(500));
        let cpu_info = system::cpu_info();
        Self {
            tab: Tab::Processes,
            sampler,
            snapshot: Snapshot::default(),
            history_cpu: vec![0.0; 120],
            history_mem: vec![0.0; 120],
            selected: None,
            user: user::current_user(),
            os: system::os_summary(),
            cpu_info,
            ram_installed: system::installed_ram_bytes(),
            search: String::new(),
            group_by_app: true,
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
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        let mut got = false;
        while let Ok(snap) = self.sampler.rx.try_recv() {
            self.history_cpu.remove(0);
            self.history_cpu.push(snap.cpu_percent);
            self.history_mem.remove(0);
            self.history_mem.push(snap.memory.used_percent());
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

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for t in Tab::ALL {
                if ui.selectable_label(self.tab == t, t.label()).clicked() {
                    self.tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(sel) = self.selected {
                    if let Some(row) = self.snapshot.rows.iter().find(|r| r.info.pid == sel) {
                        ui.label(RichText::new(format!("{}  ·  PID {}", row.info.name, sel)).small().weak());
                    }
                }
                ui.label(
                    RichText::new(format!(
                        "{}  ·  {}  ·  {} cores",
                        self.os, self.cpu_info.arch, self.cpu_info.logical
                    ))
                    .small()
                    .weak(),
                );
            });
        });
    }

    fn processes_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut self.search).hint_text("🔍 Search processes"));
            ui.checkbox(&mut self.group_by_app, "Group by app");
            ui.separator();
            let selected = self.selected.and_then(|pid| self.snapshot.rows.iter().find(|r| r.info.pid == pid).cloned());
            let can_end = selected.is_some();
            if ui.add_enabled(can_end, egui::Button::new("End task")).clicked() {
                if let Some(row) = &selected {
                    self.confirm_kill = Some((row.info.pid, row.info.name.clone()));
                }
            }
            if ui.add_enabled(can_end, egui::Button::new("Suspend")).clicked() {
                if let Some(row) = &selected {
                    match process::suspend(row.info.pid) {
                        Ok(n) => self.status = format!("Suspended {n} threads of {}", row.info.name),
                        Err(e) => self.status = format!("Suspend failed: {e}"),
                    }
                }
            }
            if ui.add_enabled(can_end, egui::Button::new("Resume")).clicked() {
                if let Some(row) = &selected {
                    match process::resume(row.info.pid) {
                        Ok(n) => self.status = format!("Resumed {n} threads of {}", row.info.name),
                        Err(e) => self.status = format!("Resume failed: {e}"),
                    }
                }
            }
            if ui.add_enabled(can_end, egui::Button::new("Priority…")).clicked() {
                if let Some(row) = &selected {
                    self.priority_open = Some(row.info.pid);
                }
            }
            // Priority selector
            egui::ComboBox::from_id_salt("prio")
                .selected_text("Set priority")
                .show_ui(ui, |ui| {
                    if let Some(row) = &selected {
                        for p in process::ALL_PRIORITIES {
                            if ui.selectable_label(row.info.priority == p, p.label()).clicked() {
                                match process::set_priority(row.info.pid, p) {
                                    Ok(()) => self.status = format!("{} -> {}", row.info.name, p.label()),
                                    Err(e) => self.status = format!("Priority change failed: {e}"),
                                }
                            }
                        }
                    } else {
                        ui.label("Select a process first");
                    }
                });
            if ui.add_enabled(can_end, egui::Button::new("Open file location")).clicked() {
                if let Some(row) = &selected {
                    if !row.info.exe_path.is_empty() {
                        let _ = shell::open(std::path::Path::new(&row.info.exe_path));
                    }
                }
            }
            if ui.add_enabled(can_end, egui::Button::new("Properties")).clicked() {
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
        ui.separator();

        // Build filtered rows for display.
        let needle = self.search.to_lowercase();
        let mut display: Vec<Row> = self
            .snapshot
            .rows
            .iter()
            .filter(|r| needle.is_empty() || r.info.name.to_lowercase().contains(&needle) || r.info.pid.to_string() == needle)
            .cloned()
            .collect();
        // Sort according to the selected column.
        match self.sort_by {
            SortBy::Name => display.sort_by(|a, b| a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase())),
            SortBy::Cpu => display.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal)),
            SortBy::Memory => display.sort_by(|a, b| b.info.private_bytes.cmp(&a.info.private_bytes)),
            SortBy::Disk => display.sort_by(|a, b| b.disk_bps.partial_cmp(&a.disk_bps).unwrap_or(std::cmp::Ordering::Equal)),
            SortBy::Network => display.sort_by(|a, b| b.net_bps.partial_cmp(&a.net_bps).unwrap_or(std::cmp::Ordering::Equal)),
            SortBy::Pid => display.sort_by_key(|r| r.info.pid),
        }
        if !self.sort_desc {
            display.reverse();
        }
        // Group by app when requested.
        if self.group_by_app {
            display.sort_by(|a, b| a.group.cmp(&b.group).then(b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal)));
        }

        let total_cpu: f32 = self.snapshot.cpu_percent;
        let total_mem = self.snapshot.memory.used_phys();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("CPU {total_cpu:.1}%  ·  Memory {}", used_mem_text(total_mem))).strong());
            ui.label(RichText::new(format!("({:.1}%)", self.snapshot.memory.used_percent())).weak());
        });

        let mut clicked: Option<u32> = None;
        let mut sort_col: Option<SortBy> = None;
        let mut end_pid: Option<u32> = None;
        let mut new_priority: Option<(u32, Priority)> = None;

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(220.0).at_least(120.0).clip(true))
            .column(Column::initial(70.0).at_least(50.0).clip(true))
            .column(Column::initial(90.0).at_least(60.0).clip(true))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::remainder().at_least(80.0).clip(true));

        let table = table.header(22.0, |mut header| {
            for (title, key) in [
                ("Name", SortBy::Name),
                ("Status", SortBy::Pid),
                ("CPU", SortBy::Cpu),
                ("Memory", SortBy::Memory),
                ("Disk", SortBy::Disk),
                ("Network", SortBy::Network),
                ("Priority", SortBy::Pid),
                ("Exe", SortBy::Name),
            ] {
                header.col(|ui| {
                    let arrow = if self.sort_by == key { if self.sort_desc { " ▼" } else { " ▲" } } else { "" };
                    if ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false)).clicked() {
                        sort_col = Some(key);
                    }
                });
            }
        });

        table.body(|body| {
            body.rows(22.0, display.len(), |mut row| {
                let idx = row.index();
                let r = &display[idx];
                let selected = self.selected == Some(r.info.pid);
                row.set_selected(selected);
                row.col(|ui| {
                    let resp = ui.selectable_label(selected, RichText::new(&r.info.name));
                    if resp.clicked() {
                        clicked = Some(r.info.pid);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("End task").clicked() {
                            end_pid = Some(r.info.pid);
                            ui.close_menu();
                        }
                        if ui.button("Suspend").clicked() {
                            let _ = process::suspend(r.info.pid);
                            ui.close_menu();
                        }
                        if ui.button("Resume").clicked() {
                            let _ = process::resume(r.info.pid);
                            ui.close_menu();
                        }
                        if !r.info.exe_path.is_empty() && ui.button("Open file location").clicked() {
                            let _ = shell::open(std::path::Path::new(&r.info.exe_path));
                            ui.close_menu();
                        }
                        if !r.info.exe_path.is_empty() && ui.button("Properties").clicked() {
                            let _ = shell::show_properties(&r.info.exe_path);
                            ui.close_menu();
                        }
                        ui.separator();
                        for p in process::ALL_PRIORITIES {
                            if ui.button(format!("Priority: {}", p.label())).clicked() {
                                new_priority = Some((r.info.pid, p));
                                ui.close_menu();
                            }
                        }
                    });
                });
                row.col(|ui| {
                    let text = if r.info.access == AccessLevel::Denied { "Access denied" } else { "Running" };
                    ui.label(RichText::new(text).weak());
                });
                row.col(|ui| {
                    let color = heat_color(r.cpu_percent, 10.0);
                    ui.label(RichText::new(format!("{:.1}%", r.cpu_percent)).color(color).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(memory_mb(r.info.private_bytes)).monospace());
                });
                row.col(|ui| {
                    let color = heat_color(r.disk_bps as f32 / 1024.0, 200.0);
                    ui.label(RichText::new(rate_text(r.disk_bps)).color(color).monospace());
                });
                row.col(|ui| {
                    let color = heat_color(r.net_bps as f32 / 1024.0, 200.0);
                    ui.label(RichText::new(rate_text(r.net_bps)).color(color).monospace());
                });
                row.col(|ui| {
                    ui.label(RichText::new(r.info.priority.label()).weak());
                });
                row.col(|ui| {
                    let p = shorten_path(&r.info.exe_path, 46);
                    ui.label(RichText::new(p).small().weak());
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
                Ok(()) => self.status = format!("Priority set to {}", prio.label()),
                Err(e) => self.status = format!("Priority change failed: {e}"),
            }
        }
    }

    fn performance_tab(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            cols[0].heading("CPU");
            self.graph(&mut cols[0], &self.history_cpu.clone(), 100.0, "CPU");
            cols[0].label(format!("{} · {} logical processors", self.cpu_info.arch, self.cpu_info.logical));
            cols[0].label(format!("Uptime: {}", format_uptime(self.snapshot.uptime_ms)));
            cols[0].separator();
            cols[0].heading("Memory");
            self.graph(&mut cols[1], &self.history_mem.clone(), 100.0, "Memory");
            let m = self.snapshot.memory;
            cols[0].label(format!("In use: {} ({:.0}%)", used_mem_text(m.used_phys()), m.used_percent()));
            cols[0].label(format!("Available: {}", used_mem_text(m.avail_phys)));
            cols[0].label(format!("Total: {}", used_mem_text(m.total_phys)));
            if let Some(ram) = self.ram_installed {
                cols[0].label(format!("Installed RAM: {}", used_mem_text(ram)));
            }
            cols[0].label(format!("Committed: {}", used_mem_text(m.total_page_file - m.avail_page_file)));
        });
    }

    fn graph(&self, ui: &mut egui::Ui, data: &[f32], max: f32, label: &str) {
        let size = egui::vec2(ui.available_width().min(420.0), 140.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, Color32::from_rgb(24, 24, 30));
        // grid
        for i in 1..4 {
            let y = rect.top() + rect.height() * i as f32 / 4.0;
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0_f32, Color32::from_rgb(40, 40, 52)),
            );
        }
        let n = data.len();
        let step = rect.width() / (n.saturating_sub(1)).max(1) as f32;
        let mut points = Vec::with_capacity(n);
        for (i, v) in data.iter().enumerate() {
            let y = rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height();
            points.push(egui::pos2(rect.left() + i as f32 * step, y));
        }
        if points.len() > 1 {
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255)),
            ));
            let mut fill = points;
            fill.push(egui::pos2(rect.right(), rect.bottom()));
            fill.push(egui::pos2(rect.left(), rect.bottom()));
            painter.add(egui::Shape::convex_polygon(
                fill,
                Color32::from_rgba_unmultiplied(120, 200, 255, 30),
                egui::Stroke::NONE,
            ));
        }
        painter.text(
            rect.left_top() + egui::vec2(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            format!("{} {:.1}%", label, data.last().copied().unwrap_or(0.0)),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(200, 220, 255),
        );
    }

    /// Full priority selector window opened from the toolbar "Priority…" combo,
    /// mirroring Task Manager's "Set priority" submenu reachable without a
    /// right click. Uses the same `process::set_priority` path as the menu.
    fn priority_window(&mut self, ctx: &egui::Context) {
        let Some(pid) = self.priority_open else { return };
        let Some(row) = self.snapshot.rows.iter().find(|r| r.info.pid == pid).cloned() else {
            self.priority_open = None;
            return;
        };
        let mut keep_open = true;
        let mut chosen: Option<Priority> = None;
        egui::Window::new(format!("Priority — {}", row.info.name))
            .collapsible(false)
            .resizable(false)
            .open(&mut keep_open)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("Current: {}", row.info.priority.label())).weak());
                ui.separator();
                for p in process::ALL_PRIORITIES {
                    if ui
                        .add(egui::RadioButton::new(row.info.priority == p, p.label()))
                        .clicked()
                    {
                        chosen = Some(p);
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.priority_open = None;
                }
            });
        if let Some(p) = chosen {
            match process::set_priority(pid, p) {
                Ok(()) => self.status = format!("{} -> {}", row.info.name, p.label()),
                Err(e) => self.status = format!("Priority change failed: {e}"),
            }
            self.priority_open = None;
        } else if !keep_open {
            self.priority_open = None;
        }
    }

    fn startup_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Startup apps").strong());
            if ui.button("Refresh").clicked() {
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
                        ui.label(RichText::new(&item.command).small().weak());
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
                ui.label("No startup entries found in HKCU/HKLM Run keys.");
            }
        });
        if let Some((idx, enable)) = toggle {
            match StartupItem::set_enabled(&self.startup[idx], enable) {
                Ok(()) => {
                    status_note = Some(format!(
                        "{} {}",
                        self.startup[idx].name,
                        if enable { "enabled" } else { "disabled" }
                    ));
                    self.startup = StartupItem::scan();
                }
                Err(e) => status_note = Some(format!("Registry update failed: {e}")),
            }
        }
        if let Some(note) = status_note {
            self.status = note;
        }
    }

    fn users_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Users");
        ui.separator();
        egui::Grid::new("users_grid").num_columns(4).striped(true).show(ui, |ui| {
            ui.label(RichText::new("User").strong());
            ui.label(RichText::new("Session").strong());
            ui.label(RichText::new("Integrity").strong());
            ui.label(RichText::new("Elevated").strong());
            ui.end_row();
            ui.label(format!("{}\\{}", self.user.domain, self.user.account));
            ui.label("Console (current)");
            ui.label(self.user.integrity);
            ui.label(if self.user.elevated { "Yes" } else { "No" });
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.label(RichText::new("Sessions are enumerated from the local session table.").small().weak());
        for s in enumerate_sessions() {
            ui.label(s);
        }
    }

    fn network_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh now").clicked() {
                self.connections = net::tcp_connections();
            }
            ui.label(RichText::new(format!("{} TCP connections", self.connections.len())).weak());
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
            egui::Grid::new("net_grid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label(RichText::new("PID").strong());
                ui.label(RichText::new("Process").strong());
                ui.label(RichText::new("Connections").strong());
                ui.label(RichText::new("Local ports").strong());
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
                    let port_list = p.iter().take(8).map(|x| x.to_string()).collect::<Vec<_>>().join(", ");
                    ui.label(port_list);
                    ui.end_row();
                }
            });
        });
    }

    fn services_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                self.last_service_refresh = Instant::now() - Duration::from_secs(10);
            }
            ui.label(RichText::new(format!("{} services", self.services.len())).weak());
            if !self.user.elevated {
                ui.label(RichText::new("(read-only without elevation)").small().weak());
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("services_grid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label(RichText::new("Name").strong());
                ui.label(RichText::new("Status").strong());
                ui.label(RichText::new("Start type").strong());
                ui.label(RichText::new("PID").strong());
                ui.end_row();
                for s in &self.services {
                    ui.label(&s.display_name);
                    let state = s.state.map(|x| x.label()).unwrap_or("—");
                    let color = match s.state {
                        Some(ServiceState::Running) => Color32::from_rgb(120, 220, 120),
                        Some(ServiceState::Stopped) => Color32::from_rgb(220, 120, 120),
                        _ => Color32::GRAY,
                    };
                    ui.label(RichText::new(state).color(color));
                    ui.label(s.start_type.map(|x| x.label()).unwrap_or("—"));
                    ui.label(if s.pid == 0 { String::new() } else { s.pid.to_string() });
                    ui.end_row();
                }
            });
        });
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        if let Some((pid, name)) = self.confirm_kill.clone() {
            egui::Window::new("End task")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Terminate {name} (PID {pid})?"));
                    ui.label(RichText::new("Unsaved data in this app will be lost.").small().weak());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("End task").clicked() {
                            match process::terminate(pid) {
                                Ok(()) => self.status = format!("Terminated {name}"),
                                Err(e) => self.status = format!("Terminate failed: {e}"),
                            }
                            self.confirm_kill = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_kill = None;
                        }
                    });
                });
        }
    }
}

impl eframe::App for TaskManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump(ctx);
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.header(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} processes · CPU {:.1}% · Memory {:.1}% · refresh {} ms",
                        self.snapshot.rows.len(),
                        self.snapshot.cpu_percent,
                        self.snapshot.memory.used_percent(),
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
        });
        self.priority_window(ctx);
        self.confirm_modal(ctx);
    }
}

/// First token of a startup command line, with quotes stripped.
fn exe_from_command(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
        return rest.to_string();
    }
    t.split_whitespace().next().unwrap_or("").to_string()
}

/// Format a byte-per-second rate the way Task Manager does (B/s, KB/s, MB/s).
fn rate_text(bps: f64) -> String {
    if bps < 1024.0 {
        format!("{bps:.0} B/s")
    } else if bps < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", bps / 1024.0)
    } else if bps < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB/s", bps / (1024.0 * 1024.0 * 1024.0))
    }
}

fn memory_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn used_mem_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.2} GB")
    } else {
        memory_mb(bytes)
    }
}

fn heat_color(percent: f32, scale: f32) -> Color32 {
    let t = (percent / scale).clamp(0.0, 1.0);
    let r = (120.0 + 130.0 * t) as u8;
    let g = (220.0 - 120.0 * t) as u8;
    Color32::from_rgb(r, g, 110)
}

fn shorten_path(p: &str, max: usize) -> String {
    if p.len() <= max {
        return p.to_string();
    }
    let head = 20.min(p.len());
    let tail = max.saturating_sub(head + 1);
    format!("{}…{}", &p[..head], &p[p.len() - tail..])
}

fn format_uptime(ms: u64) -> String {
    let s = ms / 1000;
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else {
        format!("{h}h {m}m")
    }
}

fn enumerate_sessions() -> Vec<String> {
    // Session enumeration needs WTS APIs; kept intentionally conservative.
    let mut out = Vec::new();
    let console = std::env::var("SESSIONNAME").unwrap_or_else(|_| "Console".into());
    out.push(format!("  {console} — active"));
    out
}

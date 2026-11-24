//! WinRust launcher.
//!
//! A single entry point that starts any of the reimplemented components and
//! shows live system status.  The component binaries are expected next to this
//! executable (as produced by `cargo build --release`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use winrust_core::process;
use winrust_core::service::{ScManager, ServiceState};
use winrust_core::shell;
use winrust_core::system;
use winrust_core::user;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
    Explorer,
    TaskManager,
    Services,
    SystemControl,
}

impl Component {
    fn exe(self) -> &'static str {
        match self {
            Component::Explorer => "winrust-explorer.exe",
            Component::TaskManager => "winrust-taskmgr.exe",
            Component::Services => "winrust-services.exe",
            Component::SystemControl => "winrust-sysctl.exe",
        }
    }
    fn replaces(self) -> &'static str {
        match self {
            Component::Explorer => "explorer.exe",
            Component::TaskManager => "Taskmgr.exe",
            Component::Services => "services.msc",
            Component::SystemControl => "control.exe + msconfig.exe + regedit (env part)",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Component::Explorer => "File Explorer",
            Component::TaskManager => "Task Manager",
            Component::Services => "Services",
            Component::SystemControl => "System Control",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Component::Explorer => {
                "Rust file browser: tabs, breadcrumbs, details/tiles/list views, sorting, \
                 hidden files, new folder, rename, recycle, run as admin, properties, \
                 per-folder view memory."
            }
            Component::TaskManager => {
                "Rust task manager: processes with CPU/memory, end task, suspend/resume, \
                 priority, affinity, performance graphs, startup entries, network, services."
            }
            Component::Services => {
                "Rust services.msc: full service/driver list, start/stop/pause/restart, \
                 start type incl. delayed auto-start, binary path editing, descriptions."
            }
            Component::SystemControl => {
                "Rust control utilities: OS/hardware overview, environment variables written \
                 through the registry, startup entries, power plans, kernel drivers, support report."
            }
        }
    }
    const ALL: [Component; 4] = [
        Component::Explorer,
        Component::TaskManager,
        Component::Services,
        Component::SystemControl,
    ];
}

struct LauncherApp {
    selected: Component,
    components: Vec<ComponentStatus>,
    os: String,
    user: user::UserInfo,
    memory: system::MemoryStatus,
    cpu: system::CpuInfo,
    last_sample: Instant,
    cpu_percent: f32,
    prev_cpu: system::CpuSample,
    services_running: usize,
    services_total: usize,
    last_services: Instant,
    processes: usize,
    last_proc_scan: Instant,
    status: String,
}

struct ComponentStatus {
    component: Component,
    path: Option<PathBuf>,
    running: bool,
    pid: Option<u32>,
}

fn find_component(exe: &str) -> Option<PathBuf> {
    let dirs = [
        std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
        std::env::current_dir().ok(),
    ];
    for dir in dirs.into_iter().flatten() {
        for candidate in [dir.join(exe), dir.join("target").join("release").join(exe), dir.join("target").join("debug").join(exe)] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Dev convenience: workspace root / target / release
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("release")
        .join(exe);
    if fallback.is_file() {
        return Some(fallback);
    }
    None
}

fn pid_of(exe: &str) -> Option<u32> {
    let procs = process::enumerate().ok()?;
    let target = exe.to_lowercase();
    procs.iter().find(|p| p.name.to_lowercase() == target).map(|p| p.pid)
}

impl LauncherApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let components = Component::ALL
            .iter()
            .map(|c| {
                let path = find_component(c.exe());
                let pid = path.as_ref().and_then(|_| pid_of(c.exe()));
                ComponentStatus { component: *c, path, running: pid.is_some(), pid }
            })
            .collect();
        Self {
            selected: Component::Explorer,
            components,
            os: system::os_summary(),
            user: user::current_user(),
            memory: system::memory_status(),
            cpu: system::cpu_info(),
            last_sample: Instant::now() - Duration::from_secs(5),
            cpu_percent: 0.0,
            prev_cpu: system::cpu_sample(),
            services_running: 0,
            services_total: 0,
            last_services: Instant::now() - Duration::from_secs(30),
            processes: 0,
            last_proc_scan: Instant::now() - Duration::from_secs(30),
            status: String::new(),
        }
    }

    fn sample(&mut self) {
        if self.last_sample.elapsed() >= Duration::from_millis(1000) {
            let now = system::cpu_sample();
            self.cpu_percent = system::cpu_usage_between(self.prev_cpu, now);
            self.prev_cpu = now;
            self.memory = system::memory_status();
            self.last_sample = Instant::now();
        }
        if self.last_proc_scan.elapsed() >= Duration::from_secs(3) {
            self.processes = process::enumerate().map(|p| p.len()).unwrap_or(0);
            self.last_proc_scan = Instant::now();
            for c in &mut self.components {
                let pid = pid_of(c.component.exe());
                c.running = pid.is_some();
                c.pid = pid;
            }
        }
        if self.last_services.elapsed() >= Duration::from_secs(20) {
            if let Ok(scm) = ScManager::connect() {
                if let Ok(list) = scm.enumerate() {
                    self.services_total = list.len();
                    self.services_running = list.iter().filter(|s| s.state == Some(ServiceState::Running)).count();
                }
            }
            self.last_services = Instant::now();
        }
    }

    fn launch(&mut self, c: Component) {
        let Some(status) = self.components.iter().find(|s| s.component == c) else { return };
        if status.running {
            self.status = format!("{} is already running", c.title());
            return;
        }
        let Some(path) = status.path.clone() else {
            self.status = format!("{} not found — build it with: cargo build --release -p {}", c.exe(), pkg_name(c));
            return;
        };
        match shell::execute(&path, "", None) {
            Ok(pid) => self.status = format!("Started {} (pid {:?})", c.title(), pid),
            Err(e) => self.status = format!("Failed to start {}: {e}", c.title()),
        }
    }

    fn block_with_explorer(&mut self) {
        // "Replace Explorer" is implemented as a deliberate, explicit action:
        // it starts the WinRust shell and stops the system shell, which Windows
        // restarts automatically at logon unless it is replaced system-wide.
        let Some(status) = self.components.iter().find(|s| s.component == Component::Explorer) else { return };
        let Some(path) = status.path.clone() else {
            self.status = "WinRust Explorer binary not found".into();
            return;
        };
        if let Err(e) = shell::execute(&path, "", None) {
            self.status = format!("Start failed: {e}");
            return;
        }
        match pid_of("explorer.exe") {
            Some(pid) => match process::terminate(pid) {
                Ok(()) => self.status = "WinRust Explorer started; system explorer.exe stopped (it restarts on next logon)".into(),
                Err(e) => self.status = format!("WinRust Explorer started, but stopping explorer.exe failed: {e}"),
            },
            None => self.status = "WinRust Explorer started (explorer.exe was not running)".into(),
        }
    }
}

fn pkg_name(c: Component) -> &'static str {
    match c {
        Component::Explorer => "winrust-explorer",
        Component::TaskManager => "winrust-taskmgr",
        Component::Services => "winrust-services",
        Component::SystemControl => "winrust-sysctl",
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sample();

        egui::TopBottomPanel::top("banner").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("WinRust").size(26.0).strong());
                ui.label(RichText::new("Windows components reimplemented in Rust").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&self.os).small());
                });
            });
            ui.add_space(8.0);
        });

        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "CPU {:.0}%  ·  RAM {:.1} / {:.1} GB  ·  {} cores {}  ·  {} processes  ·  {} / {} services",
                        self.cpu_percent,
                        self.memory.used_phys() as f64 / 1_073_741_824.0,
                        self.memory.total_phys as f64 / 1_073_741_824.0,
                        self.cpu.logical,
                        self.cpu.arch,
                        self.processes,
                        self.services_running,
                        self.services_total
                    ))
                    .small()
                    .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let who = format!("{}\\{}", self.user.domain, self.user.account);
                    ui.label(
                        RichText::new(format!(
                            "{} · {}{}",
                            who,
                            self.user.integrity,
                            if self.user.elevated { " · admin" } else { "" }
                        ))
                        .small()
                        .weak(),
                    );
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                cols[0].heading("Components");
                cols[0].add_space(4.0);
                let mut launch: Option<Component> = None;
                let mut select: Option<Component> = None;
                for status in &self.components {
                    let selected = self.selected == status.component;
                    let frame = egui::Frame::group(ui.style()).fill(if selected {
                        Color32::from_rgb(40, 46, 60)
                    } else {
                        Color32::from_rgb(28, 30, 36)
                    });
                    frame.show(&mut cols[0], |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(status.component.title()).strong().size(15.0));
                                ui.label(RichText::new(format!("replaces {}", status.component.replaces())).small().weak());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if status.running {
                                    ui.label(RichText::new("running").small().color(Color32::from_rgb(120, 220, 120)));
                                } else if status.path.is_none() {
                                    ui.label(RichText::new("not built").small().color(Color32::from_rgb(230, 170, 120)));
                                } else {
                                    ui.label(RichText::new("ready").small().weak());
                                }
                                let btn = ui.add_enabled(!status.running && status.path.is_some(), egui::Button::new("Launch"));
                                if btn.clicked() {
                                    launch = Some(status.component);
                                }
                            });
                        });
                    });
                    if ui
                        .add(egui::Button::new(RichText::new("Details").small()).frame(false))
                        .clicked()
                    {
                        select = Some(status.component);
                    }
                    cols[0].add_space(4.0);
                }
                if let Some(c) = select {
                    self.selected = c;
                }
                if let Some(c) = launch {
                    self.launch(c);
                }

                cols[1].heading("About the selected component");
                cols[1].add_space(4.0);
                {
                    let sel = self.selected;
                    cols[1].label(RichText::new(sel.title()).size(17.0).strong());
                    cols[1].label(RichText::new(sel.description()));
                    cols[1].add_space(6.0);
                    let path = self
                        .components
                        .iter()
                        .find(|s| s.component == sel)
                        .and_then(|s| s.path.clone());
                    match path {
                        Some(p) => {
                            cols[1].label(RichText::new(format!("Binary: {}", p.display())).small().weak());
                        }
                        None => {
                            cols[1].label(
                                RichText::new(format!("Build with: cargo build --release -p {}", pkg_name(sel)))
                                    .small()
                                    .color(Color32::from_rgb(240, 190, 110)),
                            );
                        }
                    }
                }
                cols[1].add_space(12.0);
                cols[1].separator();
                cols[1].heading("Shell takeover");
                cols[1].label(
                    RichText::new(
                        "Start WinRust Explorer and stop the system shell in one action. \
                         Windows restarts explorer.exe at next logon unless you replace it \
                         system-wide (see docs/ADR-0002-explorer-shell-replacement.md).",
                    )
                    .small(),
                );
                if cols[1].add(egui::Button::new("Start WinRust Explorer as shell")).clicked() {
                    self.block_with_explorer();
                }
                cols[1].add_space(10.0);
                if !self.status.is_empty() {
                    cols[1].label(RichText::new(&self.status).small());
                }
            });
        });

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 620.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("WinRust"),
        ..Default::default()
    };
    eframe::run_native(
        "WinRust",
        options,
        Box::new(|cc| Ok(Box::new(LauncherApp::new(cc)))),
    )
}

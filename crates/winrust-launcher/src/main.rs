//! WinRust launcher & selective installer.
//!
//! The launcher is the single entry point of the project.  It does three jobs:
//!
//! 1. **Launch** any component, showing what is running and the live system
//!    status (CPU, RAM, processes, services).
//! 2. **Install selectively** — each component can be switched between
//!    *Portable* (nothing changes), *Redirect* (IFEO hook, per-exe) and
//!    *Replace system binary* (backup + overwrite, needs admin).
//! 3. **Protect** the machine before touching anything: a System Restore point,
//!    a file-level backup of every binary about to be replaced, or both.
//!
//! Every action lands in an in-app log so the user always sees exactly what was
//! written to the system.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use winrust_core::install::{self, Integration, TargetSpec};
use winrust_core::process;
use winrust_core::restore;
use winrust_core::service::{ScManager, ServiceState};
use winrust_core::shell;
use winrust_core::system;
use winrust_core::user;

// ---------------------------------------------------------------------------
// Component catalogue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Component {
    Explorer,
    TaskManager,
    Services,
    SystemControl,
}

impl Component {
    fn id(self) -> &'static str {
        match self {
            Component::Explorer => "explorer",
            Component::TaskManager => "taskmgr",
            Component::Services => "services",
            Component::SystemControl => "sysctl",
        }
    }
    fn exe(self) -> &'static str {
        match self {
            Component::Explorer => "winrust-explorer.exe",
            Component::TaskManager => "winrust-taskmgr.exe",
            Component::Services => "winrust-services.exe",
            Component::SystemControl => "winrust-sysctl.exe",
        }
    }
    fn pkg(self) -> &'static str {
        match self {
            Component::Explorer => "winrust-explorer",
            Component::TaskManager => "winrust-taskmgr",
            Component::Services => "winrust-services",
            Component::SystemControl => "winrust-sysctl",
        }
    }
    /// The Windows executable this component stands in for.
    fn replaces(self) -> &'static str {
        match self {
            Component::Explorer => "explorer.exe",
            Component::TaskManager => "Taskmgr.exe",
            Component::Services => "services.msc",
            Component::SystemControl => "control.exe",
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
    fn glyph(self) -> &'static str {
        match self {
            Component::Explorer => "🗂",
            Component::TaskManager => "📊",
            Component::Services => "⚙",
            Component::SystemControl => "🛠",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Component::Explorer => {
                "Tabs, breadcrumbs, four view modes, multi-selection, drag & drop, \
                 preview pane, background copy/move with cancel, shell icons, \
                 Recycle Bin and permanent delete."
            }
            Component::TaskManager => {
                "Processes with live CPU, memory, disk and network per PID, \
                 end task, suspend/resume, priority, startup toggle, performance graphs, \
                 network endpoints, services."
            }
            Component::Services => {
                "Full service and driver list, start/stop/pause/resume, start type \
                 including delayed auto-start, binary path and description editing, \
                 exact Win32 errors when elevation is missing."
            }
            Component::SystemControl => {
                "OS and hardware overview, environment variables with WM_SETTINGCHANGE, \
                 startup entries, power plans, kernel drivers, support report."
            }
        }
    }
    /// `None` for components that have no single system binary to replace.
    fn target(self) -> Option<&'static str> {
        match self {
            Component::Explorer => Some("explorer.exe"),
            Component::TaskManager => Some("Taskmgr.exe"),
            Component::Services => None,
            Component::SystemControl => None,
        }
    }
    /// Integration levels offered for this component.
    fn levels(self) -> &'static [Integration] {
        match self {
            Component::Explorer => &[
                Integration::Portable,
                Integration::Shell,
                Integration::Replace,
            ],
            Component::TaskManager => &[Integration::Portable, Integration::Hijack, Integration::Replace],
            _ => &[Integration::Portable],
        }
    }

    const ALL: [Component; 4] = [
        Component::Explorer,
        Component::TaskManager,
        Component::Services,
        Component::SystemControl,
    ];
}

// ---------------------------------------------------------------------------
// Per-component install state
// ---------------------------------------------------------------------------

struct ComponentState {
    component: Component,
    path: Option<PathBuf>,
    running: bool,
    pid: Option<u32>,
    integration: Integration,
    /// Include this component in the "Install selected" batch.
    selected: bool,
    target: Option<TargetSpec>,
    /// Last backup taken for this component (for one-click undo).
    last_backup: Option<restore::BackupSession>,
    installed_at: Option<Instant>,
}

impl ComponentState {
    fn status_text(&self) -> (&'static str, Color32) {
        if self.integration != Integration::Portable {
            return ("installed", Color32::from_rgb(120, 220, 120));
        }
        if self.path.is_none() {
            return ("not built", Color32::from_rgb(230, 170, 120));
        }
        ("ready", Color32::GRAY)
    }
}

// ---------------------------------------------------------------------------
// Launcher state
// ---------------------------------------------------------------------------

struct LauncherApp {
    selected: Component,
    components: Vec<ComponentState>,
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
    log: Vec<LogLine>,
    /// Safety options for the next install.
    make_restore_point: bool,
    make_file_backup: bool,
    /// Result of the last protection step.
    protection: Option<String>,
    busy: bool,
    /// Confirmation dialog before writing to the system.
    confirm: Option<PendingAction>,
    backups: Vec<restore::BackupSession>,
    backup_scan_at: Instant,
    show_backups: bool,
}

struct LogLine {
    at: Instant,
    text: String,
    kind: LogKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogKind {
    Info,
    Ok,
    Warn,
    Error,
}

struct PendingAction {
    title: String,
    body: String,
    install: Vec<Component>,
}

impl LauncherApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let components = Component::ALL
            .iter()
            .map(|c| {
                let path = find_component(c.exe());
                let pid = path.as_ref().and_then(|_| pid_of(c.exe()));
                let target = c.target().map(TargetSpec::for_exe);
                // Persisted state wins; live registry probing is the fallback
                // for installs made by an older launcher or by hand.
                let integration = install::read_state(c.id())
                    .or_else(|| match &target {
                        Some(spec) if install::hijack_active(spec) => Some(Integration::Hijack),
                        Some(spec) if install::replacement_active(spec) => Some(Integration::Replace),
                        _ => None,
                    })
                    .unwrap_or(Integration::Portable);
                ComponentState {
                    component: *c,
                    path,
                    running: pid.is_some(),
                    pid,
                    integration,
                    selected: true,
                    target,
                    last_backup: None,
                    installed_at: None,
                }
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
            log: vec![LogLine {
                at: Instant::now(),
                text: "WinRust launcher started".into(),
                kind: LogKind::Info,
            }],
            make_restore_point: restore::restore_enabled_hint(),
            make_file_backup: true,
            protection: None,
            busy: false,
            confirm: None,
            backups: restore::list_sessions(),
            backup_scan_at: Instant::now(),
            show_backups: false,
        }
    }

    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.log.push(LogLine { at: Instant::now(), text: text.into(), kind });
        if self.log.len() > 500 {
            self.log.remove(0);
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
            // One snapshot feeds the process count *and* every component's
            // "running" badge: `enrich` opens a handle per process, so calling
            // `enumerate` once per component would cost 4x the I/O.
            let snapshot = process::enumerate().unwrap_or_default();
            self.processes = snapshot.len();
            for c in &mut self.components {
                let want = c.component.exe().to_lowercase();
                let pid = snapshot
                    .iter()
                    .find(|p| p.name.to_lowercase() == want)
                    .map(|p| p.pid);
                c.running = pid.is_some();
                c.pid = pid;
            }
            self.last_proc_scan = Instant::now();
        }
        if self.last_services.elapsed() >= Duration::from_secs(20) {
            if let Ok(scm) = ScManager::connect() {
                if let Ok(list) = scm.enumerate() {
                    self.services_total = list.len();
                    self.services_running =
                        list.iter().filter(|s| s.state == Some(ServiceState::Running)).count();
                }
            }
            self.last_services = Instant::now();
        }
        if self.backup_scan_at.elapsed() >= Duration::from_secs(10) {
            self.backups = restore::list_sessions();
            self.backup_scan_at = Instant::now();
        }
    }

    fn launch(&mut self, c: Component) {
        let snapshot = match self.components.iter().find(|s| s.component == c) {
            Some(s) => (s.running, s.path.clone()),
            None => return,
        };
        let (running, path) = snapshot;
        if running {
            self.status = format!("{} is already running", c.title());
            return;
        }
        let Some(path) = path else {
            let msg = format!("{} not found — build it with: cargo build --release -p {}", c.exe(), c.pkg());
            self.status = msg.clone();
            self.log(LogKind::Warn, msg);
            return;
        };
        match shell::execute(&path, "", None) {
            Ok(pid) => {
                let msg = format!("Started {} (pid {})", c.title(), pid.unwrap_or(0));
                self.status = msg.clone();
                self.log(LogKind::Ok, msg);
            }
            Err(e) => {
                let msg = format!("Failed to start {}: {e}", c.title());
                self.status = msg.clone();
                self.log(LogKind::Error, msg);
            }
        }
    }

    /// Ask for confirmation before touching the system.
    fn request_install(&mut self, components: Vec<Component>) {
        if components.is_empty() {
            self.status = "Select at least one component".into();
            return;
        }
        if !self.user.elevated {
            let msg = "Administrator rights are required to install. Relaunch WinRust as administrator.";
            self.status = msg.into();
            self.log(LogKind::Error, msg);
            return;
        }
        let list: Vec<String> = components
            .iter()
            .map(|c| {
                let st = self.components.iter().find(|s| s.component == *c);
                let integ = st.map(|s| s.integration).unwrap_or(Integration::Portable);
                format!("• {} — {}", c.title(), integ.label())
            })
            .collect();
        let mut body = list.join("\n");
        body.push_str("\n\n");
        if self.make_restore_point {
            body.push_str("✓ System Restore point will be created\n");
        }
        if self.make_file_backup {
            body.push_str("✓ Original files will be backed up\n");
        }
        if !self.make_restore_point && !self.make_file_backup {
            body.push_str("⚠ No backup selected — changes cannot be undone automatically\n");
        }
        self.confirm = Some(PendingAction {
            title: "Apply changes to this PC?".into(),
            body,
            install: components,
        });
    }

    /// The real work: protect → stage → install → log.
    fn perform_install(&mut self, components: Vec<Component>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.protection = None;

        // 1. System Restore point
        let mut restore_sequence = None;
        if self.make_restore_point {
            let desc = "WinRust selective install";
            let result = restore::create_restore_point(desc);
            if result.created {
                restore_sequence = Some(result.sequence);
                let msg = format!("System Restore point #{} created", result.sequence);
                self.log(LogKind::Ok, msg.clone());
                self.protection = Some(msg);
            } else {
                let msg = format!("Restore point FAILED: {}", result.message);
                self.log(LogKind::Error, msg.clone());
                self.protection = Some(msg);
            }
        }

        // 2. Stage the binaries into %ProgramFiles%\WinRust
        let mut sources: Vec<(String, PathBuf)> = Vec::new();
        for c in &components {
            if let Some(st) = self.components.iter().find(|s| s.component == *c) {
                if let Some(p) = &st.path {
                    sources.push((c.id().to_string(), p.clone()));
                }
            }
        }
        let staged = match install::stage_binaries(&sources) {
            Ok(v) => {
                self.log(
                    LogKind::Ok,
                    format!("Staged {} binary(ies) into {}", v.len(), install::install_dir().display()),
                );
                v
            }
            Err(e) => {
                self.log(LogKind::Error, format!("Staging failed: {e}"));
                self.status = format!("Install aborted: {e}");
                self.busy = false;
                return;
            }
        };

        // 3. Backup the targets that will be replaced
        let mut backup: Option<restore::BackupSession> = None;
        if self.make_file_backup {
            let paths: Vec<PathBuf> = components
                .iter()
                .filter_map(|c| self.components.iter().find(|s| s.component == *c))
                .filter_map(|s| s.target.as_ref())
                .filter_map(|t| t.system_path.clone())
                .collect();
            if !paths.is_empty() {
                match restore::backup_files(&paths, "WinRust selective install", restore_sequence) {
                    Ok(session) => {
                        self.log(
                            LogKind::Ok,
                            format!(
                                "Backed up {} file(s) to {}",
                                session.entries.len(),
                                session.directory().display()
                            ),
                        );
                        self.backups.insert(0, session.clone());
                        backup = Some(session);
                    }
                    Err(e) => {
                        self.log(LogKind::Error, format!("Backup failed: {e}"));
                        self.status = format!("Install aborted: backup failed ({e})");
                        self.busy = false;
                        return;
                    }
                }
            }
        }

        // 4. Install each component with its chosen integration.
        let mut installed = 0usize;
        let mut failed = 0usize;
        for c in &components {
            let Some(st_index) = self.components.iter().position(|s| s.component == *c) else { continue };
            let integration = self.components[st_index].integration;
            let Some(spec) = self.components[st_index].target.clone() else {
                self.log(
                    LogKind::Info,
                    format!("{}: no system binary to replace — available from the launcher", c.title()),
                );
                continue;
            };
            let staged_path = staged
                .iter()
                .find(|(id, _)| id.as_str() == c.id())
                .map(|(_, p)| p.clone())
                .or_else(|| self.components[st_index].path.clone());
            let Some(source) = staged_path else {
                self.log(LogKind::Warn, format!("{}: binary not found, skipped", c.title()));
                failed += 1;
                continue;
            };
            let report = install::install(c.title(), &source, &spec, integration);
            if report.success {
                installed += 1;
                for a in &report.actions {
                    self.log(LogKind::Ok, format!("{}: {a}", c.title()));
                }
                self.components[st_index].last_backup = backup.clone();
                self.components[st_index].installed_at = Some(Instant::now());
                if integration != Integration::Portable {
                    let target = spec.system_path.as_deref();
                    if let Err(e) = install::record_state(c.id(), integration, target) {
                        self.log(LogKind::Warn, format!("{}: state not saved ({e})", c.title()));
                    }
                } else {
                    let _ = install::clear_state(c.id());
                }
            } else {
                failed += 1;
                let msg = report.error.unwrap_or_else(|| "unknown error".into());
                self.log(LogKind::Error, format!("{}: {msg}", c.title()));
            }
        }

        self.status = format!("Installed {installed} component(s), {failed} failed");
        self.log(LogKind::Info, self.status.clone());
        self.busy = false;
    }

    fn request_uninstall(&mut self, c: Component) {
        let snapshot = match self.components.iter().find(|s| s.component == c) {
            Some(s) => (s.integration, s.target.clone(), s.last_backup.clone()),
            None => return,
        };
        let (integration, spec, session) = snapshot;
        if integration == Integration::Portable {
            self.status = format!("{} is not installed", c.title());
            return;
        }
        if !self.user.elevated {
            self.status = "Administrator rights are required to uninstall".into();
            self.log(LogKind::Error, "Uninstall needs elevation");
            return;
        }
        let backup = session.or_else(|| {
            // Fall back to the most recent session that mentions this binary.
            self.backups.iter().find(|s| {
                spec.as_ref()
                    .and_then(|sp| sp.system_path.as_ref())
                    .map(|p| s.entries.iter().any(|e| &e.original == p))
                    .unwrap_or(false)
            })
            .cloned()
        });
        let Some(spec) = spec else {
            self.status = format!("{} has no undo information", c.title());
            return;
        };
        let report = install::uninstall(c.title(), &spec, integration, backup.as_ref());
        if report.success {
            for a in &report.actions {
                self.log(LogKind::Ok, format!("{}: {a}", c.title()));
            }
            if let Some(st) = self.components.iter_mut().find(|s| s.component == c) {
                st.integration = Integration::Portable;
                st.last_backup = None;
                st.installed_at = None;
            }
            if let Err(e) = install::clear_state(c.id()) {
                self.log(LogKind::Warn, format!("{}: recorded state not cleared ({e})", c.title()));
            }
            self.status = format!("{} restored", c.title());
        } else {
            let msg = report.error.unwrap_or_else(|| "unknown error".into());
            self.log(LogKind::Error, format!("{}: uninstall failed — {msg}", c.title()));
            self.status = format!("{}: uninstall failed", c.title());
        }
    }

    fn selected_components(&self) -> Vec<Component> {
        self.components
            .iter()
            .filter(|s| s.selected)
            .map(|s| s.component)
            .collect()
    }

    fn block_with_explorer(&mut self) {
        let path = match self.components.iter().find(|s| s.component == Component::Explorer) {
            Some(st) => st.path.clone(),
            None => None,
        };
        let Some(path) = path else {
            self.status = "WinRust Explorer binary not found".into();
            return;
        };
        if let Err(e) = shell::execute(&path, "", None) {
            self.status = format!("Start failed: {e}");
            return;
        }
        match pid_of("explorer.exe") {
            Some(pid) => match process::terminate(pid) {
                Ok(()) => {
                    let msg = "WinRust Explorer started; system explorer.exe stopped (it restarts at next logon)";
                    self.status = msg.into();
                    self.log(LogKind::Ok, msg);
                }
                Err(e) => self.log(LogKind::Error, format!("stopping explorer.exe failed: {e}")),
            },
            None => self.status = "WinRust Explorer started (explorer.exe was not running)".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery helpers
// ---------------------------------------------------------------------------

fn find_component(exe: &str) -> Option<PathBuf> {
    let dirs = [
        std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
        std::env::current_dir().ok(),
        Some(install::install_dir()),
    ];
    for dir in dirs.into_iter().flatten() {
        for candidate in [
            dir.join(exe),
            dir.join("target").join("release").join(exe),
            dir.join("target").join("debug").join(exe),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
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

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sample();

        egui::TopBottomPanel::top("banner").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("WinRust").size(26.0).strong());
                ui.label(RichText::new("Windows components rewritten in Rust").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, text) = if self.user.elevated {
                        (Color32::from_rgb(120, 220, 120), "administrator")
                    } else {
                        (Color32::from_rgb(230, 170, 120), "standard user — install disabled")
                    };
                    ui.label(RichText::new(text).small().color(color));
                    ui.separator();
                    ui.label(RichText::new(self.os.as_str()).small().weak());
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
                    ui.label(
                        RichText::new(format!("{} · {}", self.user.domain, self.user.account))
                            .small()
                            .weak(),
                    );
                });
            });
        });

        egui::SidePanel::right("side")
            .resizable(true)
            .default_width(400.0)
            .width_range(320.0..=620.0)
            .show(ctx, |ui| self.side_panel(ui));

        egui::CentralPanel::default().show(ctx, |ui| self.main_panel(ui));

        self.confirm_modal(ctx);
        self.backups_modal(ctx);

        ctx.request_repaint_after(Duration::from_millis(400));
    }
}

impl LauncherApp {
    fn main_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Components");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Install selected").clicked() {
                    let list = self.selected_components();
                    self.request_install(list);
                }
                if ui.button("Explore backups").clicked() {
                    self.show_backups = true;
                }
            });
        });
        ui.label(
            RichText::new(
                "Tick the components to include, choose how each one integrates, then install. \
                 Nothing is written to Windows until you confirm.",
            )
            .small()
            .weak(),
        );
        ui.add_space(6.0);

        let mut launch: Option<Component> = None;
        let mut select: Option<Component> = None;
        let mut uninstall: Option<Component> = None;
        let mut toggle_selected: Option<Component> = None;
        let mut change_integration: Option<(Component, Integration)> = None;
        let elevated = self.user.elevated;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for idx in 0..self.components.len() {
                let (component, running, integration, path_ok) = {
                    let st = &self.components[idx];
                    (st.component, st.running, st.integration, st.path.is_some())
                };
                let selected = self.selected == component;
                let (status_label, status_color) = self.components[idx].status_text();
                let frame = egui::Frame::group(ui.style())
                    .fill(if selected { Color32::from_rgb(40, 46, 60) } else { Color32::from_rgb(28, 30, 36) });
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Batch checkbox
                        let mut ticked = self.components[idx].selected;
                        if ui.checkbox(&mut ticked, "").changed() {
                            toggle_selected = Some(component);
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{}  {}", component.glyph(), component.title())).strong().size(15.0));
                                ui.label(RichText::new(status_label).small().color(status_color));
                                if running {
                                    ui.label(RichText::new("running").small().color(Color32::from_rgb(120, 220, 120)));
                                }
                            });
                            ui.label(
                                RichText::new(format!("replaces {}", component.replaces()))
                                    .small()
                                    .weak(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if integration == Integration::Portable {
                                if ui
                                    .add_enabled(!running && path_ok, egui::Button::new("Launch"))
                                    .clicked()
                                {
                                    launch = Some(component);
                                }
                            } else if ui.button("Restore").on_hover_text("Undo the installation").clicked() {
                                uninstall = Some(component);
                            }
                            if ui.button("Details").clicked() {
                                select = Some(component);
                            }
                        });
                    });

                    // Integration selector
                    if self.components[idx].target.is_some() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Integration:").small().weak());
                            for &level in component.levels() {
                                let enabled = !level.requires_admin() || elevated;
                                let resp = ui.add_enabled(
                                    enabled && !self.busy,
                                    egui::RadioButton::new(integration == level, level.label()),
                                );
                                if resp.on_hover_text(level.hint()).clicked() {
                                    change_integration = Some((component, level));
                                }
                            }
                        });
                    } else {
                        ui.label(
                            RichText::new(
                                "No single system binary — this component runs from the launcher.",
                            )
                            .small()
                            .weak(),
                        );
                    }
                });
                ui.add_space(4.0);
            }
        });

        if let Some(c) = select {
            self.selected = c;
        }
        if let Some(c) = toggle_selected {
            if let Some(st) = self.components.iter_mut().find(|s| s.component == c) {
                st.selected = !st.selected;
            }
        }
        if let Some((c, level)) = change_integration {
            if let Some(st) = self.components.iter_mut().find(|s| s.component == c) {
                st.integration = level;
            }
            self.status = format!("{}: {}", c.title(), level.label());
        }
        if let Some(c) = launch {
            self.launch(c);
        }
        if let Some(c) = uninstall {
            self.request_uninstall(c);
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // ---- Safety -------------------------------------------------
            ui.heading("Before installing");
            ui.label(
                RichText::new("Every change can be undone. Choose at least one safety net.")
                    .small()
                    .weak(),
            );
            ui.add_space(4.0);
            let mut restore_pt = self.make_restore_point;
            let hint = if restore::restore_enabled_hint() {
                "System Restore point"
            } else {
                "System Restore point (may be disabled by policy)"
            };
            if ui.checkbox(&mut restore_pt, hint).changed() {
                self.make_restore_point = restore_pt;
            }
            ui.label(
                RichText::new("Snapshot Windows itself; can be rolled back from Control Panel.")
                    .small()
                    .weak(),
            );
            let mut backup = self.make_file_backup;
            if ui.checkbox(&mut backup, "Back up replaced files").changed() {
                self.make_file_backup = backup;
            }
            ui.label(
                RichText::new("Copies the exact binaries that get overwritten into %ProgramData%\\WinRust\\backup.")
                    .small()
                    .weak(),
            );
            if let Some(p) = &self.protection {
                ui.label(RichText::new(p.as_str()).small().color(Color32::from_rgb(140, 200, 255)));
            }
            ui.separator();

            // ---- Selected component details ------------------------------
            let sel = self.selected;
            ui.horizontal(|ui| {
                ui.heading(RichText::new(format!("{}  {}", sel.glyph(), sel.title())).size(17.0));
            });
            ui.label(RichText::new(sel.description()).small());
            ui.add_space(4.0);
            let idx = self.components.iter().position(|s| s.component == sel);
            if let Some(idx) = idx {
                let st = &self.components[idx];
                match &st.path {
                    Some(p) => {
                        ui.label(RichText::new(format!("Binary: {}", p.display())).small().weak());
                    }
                    None => {
                        ui.label(
                            RichText::new(format!("Build with: cargo build --release -p {}", sel.pkg()))
                                .small()
                                .color(Color32::from_rgb(240, 190, 110)),
                        );
                    }
                }
                if let Some(t) = &st.target {
                    ui.label(
                        RichText::new(format!(
                            "Target: {}",
                            t.system_path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| format!("{} (not found)", t.exe))
                        ))
                        .small()
                        .weak(),
                    );
                    ui.label(RichText::new(format!("IFEO key: HKLM\\{}", t.ifeo_path)).small().weak());
                }
                if let Some(at) = st.installed_at {
                    ui.label(
                        RichText::new(format!("Installed {}s ago in this session", at.elapsed().as_secs()))
                            .small()
                            .weak(),
                    );
                }
                if let Some(b) = &st.last_backup {
                    ui.label(
                        RichText::new(format!(
                            "Backup: {} ({} files)",
                            b.directory().display(),
                            b.entries.len()
                        ))
                        .small()
                        .weak(),
                    );
                }
            }
            ui.separator();

            // ---- Shell takeover ------------------------------------------
            ui.heading("Shell takeover");
            ui.label(
                RichText::new(
                    "Start WinRust Explorer and stop the system shell in one action. \
                     Windows restarts explorer.exe at next logon unless you install the \
                     replacement above.",
                )
                .small(),
            );
            if ui.button("Start WinRust Explorer as shell").clicked() {
                self.block_with_explorer();
            }
            ui.separator();

            // ---- Log -----------------------------------------------------
            ui.horizontal(|ui| {
                ui.heading("Activity log");
                if ui.small_button("Clear").clicked() {
                    self.log.clear();
                }
            });
            for line in self.log.iter().rev().take(120) {
                let color = match line.kind {
                    LogKind::Info => Color32::GRAY,
                    LogKind::Ok => Color32::from_rgb(120, 220, 120),
                    LogKind::Warn => Color32::from_rgb(240, 200, 120),
                    LogKind::Error => Color32::from_rgb(240, 130, 120),
                };
                let secs = line.at.elapsed().as_secs();
                ui.label(
                    RichText::new(format!("[{}s] {}", secs, line.text))
                        .small()
                        .color(color),
                );
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.label(RichText::new(self.status.as_str()).small().strong());
            }
        });
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.confirm.as_mut() else { return };
        let mut go = false;
        let mut cancel = false;
        egui::Window::new(pending.title.as_str())
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(pending.body.as_str()).monospace().size(12.0));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Apply").strong()).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if go {
            if let Some(pending) = self.confirm.take() {
                self.perform_install(pending.install);
            }
        } else if cancel {
            self.confirm = None;
        }
    }

    fn backups_modal(&mut self, ctx: &egui::Context) {
        if !self.show_backups {
            return;
        }
        let mut open = true;
        let mut restore_now: Option<restore::BackupSession> = None;
        egui::Window::new("Backups")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .default_height(420.0)
            .show(ctx, |ui| {
                if self.backups.is_empty() {
                    ui.label("No backups yet. Backups appear here after the first install.");
                    return;
                }
                ui.label(
                    RichText::new(format!("{} session(s) in {}", self.backups.len(), restore::backup_root().display()))
                        .small()
                        .weak(),
                );
                ui.separator();
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for session in &self.backups {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("#{}", session.created_unix)).monospace());
                            ui.label(RichText::new(session.label.as_str()));
                            if let Some(seq) = session.restore_sequence {
                                ui.label(RichText::new(format!("restore pt #{seq}")).small().weak());
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Restore").clicked() {
                                    restore_now = Some(session.clone());
                                }
                            });
                        });
                        for entry in &session.entries {
                            let mark = if entry.existed { "•" } else { "✚" };
                            ui.label(
                                RichText::new(format!(
                                    "   {mark} {}  ({} B)",
                                    entry.original.display(),
                                    entry.size
                                ))
                                .small()
                                .weak(),
                            );
                        }
                        ui.separator();
                    }
                });
                ui.label(
                    RichText::new("Restoring copies the saved files back and removes files that did not exist before.")
                        .small()
                        .weak(),
                );
            });
        if let Some(session) = restore_now {
            match restore::restore_session(&session) {
                Ok(log) => {
                    self.log(LogKind::Ok, format!("Restored backup #{}", session.created_unix));
                    for line in log {
                        self.log(LogKind::Info, line);
                    }
                }
                Err(e) => self.log(LogKind::Error, format!("Restore failed: {e}")),
            }
        }
        self.show_backups = open;
    }
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("WinRust — launcher & installer"),
        ..Default::default()
    };
    eframe::run_native(
        "WinRust",
        options,
        Box::new(|cc| Ok(Box::new(LauncherApp::new(cc)))),
    )
}

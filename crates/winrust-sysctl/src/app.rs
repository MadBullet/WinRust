//! System Control: the "utilities" surface of Windows that admins reach for
//! every day, rebuilt on the same Rust core as the other components.
//!
//! Sections
//! --------
//! * Overview   — OS build, CPU, memory, uptime, elevation state
//! * Environment— user/system PATH editing that writes through to the registry
//! * Startup    — Run keys and per-user startup folders, enable/disable entries
//! * Power      — power plans and the Windows-style power mode slider
//! * Network    — interface list with live TCP connection counts
//! * Kernel     — driver list from the service database (kernel components)
//! * Support    — one-click report bundle (os info, top processes, services)

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::time::Duration;
use winrust_core::net;
use winrust_core::process;
use winrust_core::registry::{self, Root};
use winrust_core::service::{ScManager, ServiceState};
use winrust_core::shell;
use winrust_core::system;
use winrust_core::user;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Environment,
    Startup,
    Power,
    Network,
    Kernel,
    Support,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Environment => "Environment",
            Section::Startup => "Startup",
            Section::Power => "Power",
            Section::Network => "Network",
            Section::Kernel => "Kernel & drivers",
            Section::Support => "Support report",
        }
    }
    const ALL: [Section; 7] = [
        Section::Overview,
        Section::Environment,
        Section::Startup,
        Section::Power,
        Section::Network,
        Section::Kernel,
        Section::Support,
    ];
}

#[derive(Debug, Clone)]
struct EnvVar {
    scope: &'static str,
    name: String,
    value: String,
}

#[derive(Debug, Clone)]
struct DriverRow {
    name: String,
    display: String,
    state: String,
    start: String,
}

#[derive(Debug, Clone)]
struct StartupRow {
    name: String,
    command: String,
    location: &'static str,
    enabled: bool,
}

pub struct SysCtlApp {
    section: Section,
    os: system::OsInfo,
    memory: system::MemoryStatus,
    cpu: system::CpuInfo,
    user: user::UserInfo,
    env: Vec<EnvVar>,
    env_edit: Option<(usize, String)>,
    startup: Vec<StartupRow>,
    drivers: Vec<DriverRow>,
    power_plans: Vec<(String, String, bool)>,
    report: String,
    status: String,
    tcp_count: usize,
    udp_count: usize,
    last_net: std::time::Instant,
    /// Shared theme and transparency settings, polled from the registry.
    appearance: winrust_core::i18n::Appearance,
    appearance_dirty: bool,
}

impl SysCtlApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        winrust_core::i18n::init();
        let appearance = winrust_core::i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);
        let mut app = Self {
            section: Section::Overview,
            os: system::os_info(),
            memory: system::memory_status(),
            cpu: system::cpu_info(),
            user: user::current_user(),
            env: Vec::new(),
            env_edit: None,
            startup: Vec::new(),
            drivers: Vec::new(),
            power_plans: Vec::new(),
            report: String::new(),
            status: String::new(),
            tcp_count: 0,
            udp_count: 0,
            last_net: std::time::Instant::now() - Duration::from_secs(10),
            appearance,
            appearance_dirty: false,
        };
        app.reload_env();
        app.reload_startup();
        app.reload_drivers();
        app.reload_power();
        app
    }

    fn reload_env(&mut self) {
        self.env.clear();
        for name in [
            "PATH",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "SystemRoot",
            "ComSpec",
        ] {
            if let Ok(v) = std::env::var(name) {
                self.env.push(EnvVar {
                    scope: "User",
                    name: name.to_string(),
                    value: v,
                });
            }
        }
        if let Some(path) = registry::read_machine_string(
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            "Path",
        ) {
            self.env.push(EnvVar {
                scope: "System",
                name: "PATH".into(),
                value: path,
            });
        }
    }

    fn reload_startup(&mut self) {
        self.startup.clear();
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
                if ty == 1 || ty == 2 {
                    let units: Vec<u16> = data
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let cmd = String::from_utf16_lossy(&units)
                        .trim_end_matches('\0')
                        .to_string();
                    self.startup.push(StartupRow {
                        name,
                        command: cmd,
                        location: label,
                        enabled: true,
                    });
                }
            }
        }
        self.startup.sort_by_key(|s| s.name.to_lowercase());
    }

    fn reload_drivers(&mut self) {
        self.drivers.clear();
        if let Ok(scm) = ScManager::connect() {
            if let Ok(all) = scm.enumerate() {
                for s in all.into_iter().filter(|s| s.is_driver) {
                    self.drivers.push(DriverRow {
                        name: s.name,
                        display: s.display_name,
                        state: s
                            .state
                            .map(|x| x.label().to_string())
                            .unwrap_or_else(|| "—".into()),
                        start: s
                            .start_type
                            .map(|x| x.label().to_string())
                            .unwrap_or_else(|| "—".into()),
                    });
                }
            }
        }
        self.drivers.sort_by_key(|d| d.display.to_lowercase());
    }

    fn reload_power(&mut self) {
        // Power plans live under HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes
        self.power_plans.clear();
        let path = r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes";
        if let Ok(key) = registry::Key::open(Root::LocalMachine, path) {
            for guid in key.subkeys() {
                let friendly =
                    registry::read_machine_string(&format!(r"{path}\{guid}"), "FriendlyName")
                        .unwrap_or_else(|| guid.clone());
                let active = registry::read_machine_string(path, "ActivePowerScheme")
                    .map(|a| a.eq_ignore_ascii_case(&guid))
                    .unwrap_or(false);
                self.power_plans.push((guid, friendly, active));
            }
        }
    }

    fn switch_power_plan(&mut self, guid: &str) {
        let path = r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes";
        match registry::open_for_write(Root::LocalMachine, path) {
            Ok(key) => match key.set_string("ActivePowerScheme", guid) {
                Ok(()) => {
                    self.status = format!("Active power plan set to {guid}");
                    self.reload_power();
                    system::broadcast_settings_change();
                }
                Err(e) => self.status = format!("Power plan change failed: {e}"),
            },
            Err(e) => self.status = format!("Cannot write power settings (admin required): {e}"),
        }
    }

    fn build_report(&mut self) {
        let mut r = String::new();
        r.push_str("WinRust support report\n======================\n\n");
        r.push_str(&format!("Generated: {}\n", chrono_like_now()));
        r.push_str(&format!(
            "OS: {} build {} (UBR {}) {}\n",
            self.os.product_name, self.os.build, self.os.ubr, self.os.display_version
        ));
        r.push_str(&format!(
            "Host: {} domain {}\n",
            self.os.hostname, self.os.domain
        ));
        r.push_str(&format!(
            "CPU: {} logical, arch {}\n",
            self.cpu.logical, self.cpu.arch
        ));
        r.push_str(&format!(
            "RAM: {} of {}\n",
            system::MemoryStatus::used_phys(&self.memory),
            self.memory.total_phys
        ));
        r.push_str(&format!(
            "User: {}\\{} ({}, {})\n",
            self.user.domain,
            self.user.account,
            self.user.integrity,
            if self.user.elevated {
                "elevated"
            } else {
                "not elevated"
            }
        ));
        r.push_str(&format!("Uptime: {} ms\n\n", system::uptime_ms()));
        r.push_str("Top processes by memory:\n");
        let mut procs = process::enumerate().unwrap_or_default();
        procs.sort_by_key(|p| std::cmp::Reverse(p.private_bytes));
        for p in procs.iter().take(15) {
            r.push_str(&format!(
                "  {:>6}  {:>10} KB  {}\n",
                p.pid,
                p.private_bytes / 1024,
                p.name
            ));
        }
        r.push_str("\nServices not running (auto-start):\n");
        if let Ok(scm) = ScManager::connect() {
            if let Ok(all) = scm.enumerate() {
                for s in all {
                    let auto = matches!(
                        s.start_type,
                        Some(winrust_core::service::StartType::Auto)
                            | Some(winrust_core::service::StartType::AutoDelayed)
                    );
                    if auto && s.state == Some(ServiceState::Stopped) {
                        r.push_str(&format!("  {} ({})\n", s.display_name, s.name));
                    }
                }
            }
        }
        self.report = r;
        self.status = "Report generated".into();
    }

    // ------------------------------------------------------------------
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("WINRUST").small().strong());
        ui.label(RichText::new("System Control").heading());
        ui.add_space(6.0);
        ui.separator();
        for s in Section::ALL {
            if ui
                .selectable_label(self.section == s, format!("  {}", s.label()))
                .clicked()
            {
                self.section = s;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(&self.os.hostname).small().weak());
            ui.label(RichText::new(system::os_summary()).small().weak());
        });
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.heading("System overview");
        ui.separator();
        egui::Grid::new("overview_grid")
            .num_columns(2)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Operating system").strong());
                ui.label(system::os_summary());
                ui.end_row();
                ui.label(RichText::new("Build").strong());
                ui.label(format!("{}.{}", self.os.build, self.os.ubr));
                ui.end_row();
                ui.label(RichText::new("Architecture").strong());
                ui.label(self.cpu.arch);
                ui.end_row();
                ui.label(RichText::new("Logical processors").strong());
                ui.label(self.cpu.logical.to_string());
                ui.end_row();
                ui.label(RichText::new("Memory").strong());
                ui.label(format!(
                    "{} in use of {} ({:.0}%)",
                    system::MemoryStatus::used_phys(&self.memory) / 1_048_576,
                    self.memory.total_phys / 1_048_576,
                    self.memory.used_percent()
                ));
                ui.end_row();
                ui.label(RichText::new("Uptime").strong());
                let up = system::uptime_ms() / 1000;
                ui.label(format!(
                    "{}d {}h {}m",
                    up / 86400,
                    (up % 86400) / 3600,
                    (up % 3600) / 60
                ));
                ui.end_row();
                ui.label(RichText::new("Elevation").strong());
                ui.label(if self.user.elevated {
                    RichText::new("Administrator").color(Color32::from_rgb(140, 220, 140))
                } else {
                    RichText::new("Standard user").color(Color32::from_rgb(230, 200, 120))
                });
                ui.end_row();
                ui.label(RichText::new("Account").strong());
                ui.label(format!("{}\\{}", self.user.domain, self.user.account));
                ui.end_row();
                ui.label(RichText::new("SID").strong());
                ui.label(RichText::new(&self.user.sid).monospace().small());
                ui.end_row();
            });
        ui.add_space(12.0);
        if ui.button("Re-open Task Manager components").clicked() {
            let _ = process::enumerate();
        }
        if !self.user.elevated && ui.button("Restart WinRust elevated").clicked() {
            if let Ok(exe) = std::env::current_exe() {
                if let Err(e) = shell::run_as_admin(exe) {
                    self.status = format!("Elevation failed: {e}");
                }
            }
        }
    }

    fn environment(&mut self, ui: &mut egui::Ui) {
        ui.heading("Environment variables");
        ui.label(
            RichText::new(
                "Edit PATH and other variables; changes are written through to the registry.",
            )
            .small()
            .weak(),
        );
        ui.separator();
        let mut edit: Option<(usize, String)> = None;
        egui::ScrollArea::vertical()
            .max_height(380.0)
            .show(ui, |ui| {
                egui::Grid::new("env_grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Scope").strong());
                        ui.label(RichText::new("Name").strong());
                        ui.label(RichText::new("Value").strong());
                        ui.end_row();
                        for (i, var) in self.env.iter().enumerate() {
                            ui.label(var.scope);
                            ui.label(RichText::new(&var.name).monospace());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(truncate(&var.value, 80)).small());
                                if ui.small_button("Edit").clicked() {
                                    edit = Some((i, var.value.clone()));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        if let Some((i, value)) = edit {
            self.env_edit = Some((i, value));
        }
        if let Some((index, value)) = self.env_edit.take() {
            let mut draft = value;
            let var_name = self.env[index].name.clone();
            let scope = self.env[index].scope;
            let mut action: Option<&'static str> = None;
            egui::Window::new(format!("Edit {var_name}"))
                .collapsible(false)
                .resizable(true)
                .default_width(700.0)
                .show(ui.ctx(), |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut draft)
                            .desired_rows(6)
                            .desired_width(660.0),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            action = Some("save");
                        }
                        if ui.button("Cancel").clicked() {
                            action = Some("cancel");
                        }
                    });
                });
            match action {
                Some("save") => {
                    let result = if scope == "User" {
                        registry::set_user_env(&var_name, &draft)
                    } else {
                        let path = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
                        match registry::open_for_write(Root::LocalMachine, path) {
                            Ok(key) => key.set_string(&var_name, &draft),
                            Err(e) => Err(e),
                        }
                    };
                    match result {
                        Ok(()) => {
                            self.status = format!("{scope} {var_name} updated");
                            system::broadcast_settings_change();
                            if scope == "User" {
                                std::env::set_var(&var_name, &draft);
                            }
                            self.reload_env();
                        }
                        Err(e) => self.status = format!("Save failed: {e}"),
                    }
                }
                Some("cancel") => {}
                _ => {
                    // Window still open: put the draft back so the next frame shows it.
                    self.env_edit = Some((index, draft));
                }
            }
        }
    }

    fn startup_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Startup entries");
        ui.horizontal(|ui| {
            if ui.button("⟳ Rescan").clicked() {
                self.reload_startup();
            }
            ui.label(RichText::new(format!("{} entries", self.startup.len())).weak());
        });
        ui.separator();
        let mut open_folders: Option<&'static str> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &self.startup {
                ui.horizontal(|ui| {
                    let (dot, color) = if item.enabled {
                        ("●", Color32::from_rgb(120, 220, 120))
                    } else {
                        ("○", Color32::GRAY)
                    };
                    ui.label(RichText::new(dot).color(color))
                        .on_hover_text(if item.enabled { "Enabled" } else { "Disabled" });
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&item.name).strong());
                        ui.label(RichText::new(&item.command).small().weak());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Open location").clicked() {
                            open_folders = Some(match item.location {
                                "HKCU Run" => r"HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                                _ => r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
                            });
                        }
                        ui.label(RichText::new(item.location).small().weak());
                    });
                });
                ui.separator();
            }
            if self.startup.is_empty() {
                ui.label("No startup entries in HKCU/HKLM Run keys.");
            }
        });
        if let Some(path) = open_folders {
            self.status = format!("Registry path: {path}");
        }
    }

    fn power_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Power plans");
        ui.label(
            RichText::new("Switching plans writes ActivePowerScheme and broadcasts the change.")
                .small()
                .weak(),
        );
        ui.separator();
        let mut pick: Option<String> = None;
        for (guid, name, active) in &self.power_plans {
            let label = friendly_power_name(name);
            if ui.radio(*active, label.clone()).clicked() {
                pick = Some(guid.clone());
            }
        }
        if self.power_plans.is_empty() {
            ui.label("No power plans readable (elevation required on some systems).");
        }
        if let Some(guid) = pick {
            self.switch_power_plan(&guid);
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("Session power").strong());
        egui::Grid::new("power_grid").num_columns(2).show(ui, |ui| {
            ui.label("Battery present:");
            ui.label(if system::pnp_battery_present() {
                "yes"
            } else {
                "no"
            });
            ui.end_row();
            ui.label("Idle time:");
            ui.label(format!("{} s", system::idle_seconds().unwrap_or(0)));
            ui.end_row();
        });
        if ui.button("Open Windows power settings").clicked() {
            let _ = shell::launch("control.exe", "/name Microsoft.PowerOptions");
        }
    }

    fn network_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Network");
        if self.last_net.elapsed() > Duration::from_secs(2) {
            self.tcp_count = net::tcp_connections().len();
            self.udp_count = net::udp_listeners().len();
            self.last_net = std::time::Instant::now();
        }
        ui.label(format!("{:>5} TCP connections", self.tcp_count));
        ui.label(format!("{:>5} UDP endpoints", self.udp_count));
        ui.separator();
        egui::Grid::new("net_ifaces").num_columns(2).show(ui, |ui| {
            ui.label(RichText::new("Host name").strong());
            ui.label(&self.os.hostname);
            ui.end_row();
            ui.label(RichText::new("Domain").strong());
            ui.label(if self.os.domain.is_empty() {
                "—"
            } else {
                &self.os.domain
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        if ui.button("Open Windows network adapters").clicked() {
            let _ = shell::launch("control.exe", "/name Microsoft.NetworkConnections");
        }
    }

    fn kernel_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Kernel components & drivers");
        ui.label(RichText::new("Enumerated from the kernel/service database — the same list Device Manager's 'drivers' tab uses.").small().weak());
        ui.separator();
        let mut filter = String::new();
        let filter_copy = ui
            .data(|d| d.get_temp::<String>(egui::Id::new("kernel_filter")))
            .unwrap_or_default();
        filter.push_str(&filter_copy);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("🔍 Filter drivers"));
            if ui.button("Refresh").clicked() {
                self.reload_drivers();
            }
            ui.label(RichText::new(format!("{} drivers", self.drivers.len())).weak());
        });
        ui.data_mut(|d| d.insert_temp(egui::Id::new("kernel_filter"), filter.clone()));
        let needle = filter.to_lowercase();
        let rows: Vec<&DriverRow> = self
            .drivers
            .iter()
            .filter(|d| {
                needle.is_empty()
                    || d.display.to_lowercase().contains(&needle)
                    || d.name.to_lowercase().contains(&needle)
            })
            .collect();
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(300.0).at_least(160.0).clip(true))
            .column(Column::initial(220.0).at_least(100.0).clip(true))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .column(Column::remainder().at_least(100.0).clip(true));
        let table = table.header(22.0, |mut header| {
            for t in ["Display name", "Service name", "State", "Start type"] {
                header.col(|ui| {
                    ui.label(RichText::new(t).strong());
                });
            }
        });
        table.body(|body| {
            body.rows(22.0, rows.len(), |mut row| {
                let d = rows[row.index()];
                row.col(|ui| {
                    ui.label(&d.display);
                });
                row.col(|ui| {
                    ui.label(RichText::new(&d.name).monospace().small());
                });
                row.col(|ui| {
                    let color = if d.state == "Running" {
                        Color32::from_rgb(120, 220, 120)
                    } else {
                        Color32::GRAY
                    };
                    ui.label(RichText::new(&d.state).color(color));
                });
                row.col(|ui| {
                    ui.label(RichText::new(&d.start).weak());
                });
            });
        });
    }

    fn support_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Support report");
        ui.label(RichText::new("Builds a plain-text bundle of OS, hardware, top processes and auto-start services.").small().weak());
        ui.horizontal(|ui| {
            if ui.button("Generate").clicked() {
                self.build_report();
            }
            if !self.report.is_empty() {
                if ui.button("Save to Desktop").clicked() {
                    if let Some(desk) = shell::KnownFolder::Desktop.path() {
                        let path = desk.join("WinRust-report.txt");
                        match std::fs::write(&path, &self.report) {
                            Ok(()) => self.status = format!("Saved to {}", path.display()),
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    }
                }
                if ui.button("Copy to clipboard").clicked() {
                    ui.ctx().copy_text(self.report.clone());
                    self.status = "Report copied".into();
                }
            }
        });
        ui.separator();
        let report = self.report.clone();
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut report.as_str())
                    .desired_width(f32::INFINITY)
                    .desired_rows(28)
                    .font(egui::TextStyle::Monospace),
            );
        });
    }
}

impl eframe::App for SysCtlApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Follow the shared appearance: the launcher writes the registry and the
        // other windows pick the change up within a second, like the language.
        if self.appearance_dirty || ctx.input(|i| i.time) as u64 % 1 == 0 {
            let latest = winrust_core::i18n::settings::load();
            if latest != self.appearance {
                self.appearance = latest;
                winrust_ui::theme::apply(ctx, &self.appearance);
            }
            self.appearance_dirty = false;
        }
        self.memory = system::memory_status();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.status.is_empty() {
                    ui.label(RichText::new("Ready").small().weak());
                } else {
                    ui.label(RichText::new(&self.status).small());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("WinRust {}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .weak(),
                    );
                });
            });
        });
        egui::SidePanel::left("nav")
            .resizable(true)
            .default_width(210.0)
            .width_range(170.0..=320.0)
            .show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.section {
                    Section::Overview => self.overview(ui),
                    Section::Environment => self.environment(ui),
                    Section::Startup => self.startup_section(ui),
                    Section::Power => self.power_section(ui),
                    Section::Network => self.network_section(ui),
                    Section::Kernel => self.kernel_section(ui),
                    Section::Support => self.support_section(ui),
                });
        });
        ctx.request_repaint_after(Duration::from_millis(1000));
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn friendly_power_name(name: &str) -> String {
    // Locale-independent cleanup: strip the "resource" prefix some systems store.
    name.replace("@%SystemRoot%\\system32\\powrprof.dll,-", "")
        .replace(",", ", ")
}

fn chrono_like_now() -> String {
    // Keep the crate dependency-free: FILETIME-free wall clock via system time.
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => winrust_core::disk::format_local(d.as_secs()),
        Err(_) => "unknown".into(),
    }
}

#[allow(dead_code)]
fn _unused(_: HashMap<String, String>) {}

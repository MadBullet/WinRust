//! Services Manager UI: services.msc feature parity.
//!
//! * list of Win32 services + kernel drivers (grouped)
//! * filter, status columns, detail pane with description, path, logon account
//! * start / stop / pause / resume / restart
//! * properties dialog: start type (incl. delayed auto-start), binary path
//! * all actions surface the exact Win32 error when elevation is missing

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::time::{Duration, Instant};
use winrust_core::service::{ScManager, ServiceAction, ServiceEntry, ServiceState, StartType};
use winrust_core::user;

pub struct ServicesApp {
    scm: Option<ScManager>,
    services: Vec<ServiceEntry>,
    selected: Option<String>,
    filter: String,
    status: String,
    last_refresh: Instant,
    user: user::UserInfo,
    props: Option<PropsDialog>,
    action_busy: bool,
}

struct PropsDialog {
    name: String,
    display_name: String,
    start_type: StartType,
    binary_path: String,
    description: String,
    logon_account: String,
}

impl ServicesApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let user = user::current_user();
        let scm = ScManager::connect().ok();
        let mut app = Self {
            scm,
            services: Vec::new(),
            selected: None,
            filter: String::new(),
            status: String::new(),
            last_refresh: Instant::now() - Duration::from_secs(30),
            user,
            props: None,
            action_busy: false,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        match &self.scm {
            Some(scm) => match scm.enumerate() {
                Ok(list) => {
                    self.services = list;
                    self.status.clear();
                }
                Err(e) => self.status = format!("Enumerate failed: {e}"),
            },
            None => {
                self.scm = ScManager::connect().ok();
                if self.scm.is_none() {
                    self.status = "Cannot open the Service Control Manager — run WinRust elevated.".into();
                }
            }
        }
    }

    fn selected_entry(&self) -> Option<&ServiceEntry> {
        let name = self.selected.as_ref()?;
        self.services.iter().find(|s| &s.name == name)
    }

    fn do_action(&mut self, action: ServiceAction) {
        let Some(entry) = self.selected_entry().cloned() else {
            self.status = "Select a service first".into();
            return;
        };
        match winrust_core::service::control(&entry.name, action) {
            Ok(()) => {
                self.status = format!("{}: {} requested", entry.display_name, action.label());
                // Give the SCM a moment, then refresh so state is current.
                std::thread::sleep(Duration::from_millis(120));
                self.refresh();
            }
            Err(e) => self.status = format!("{}: {e}", entry.display_name),
        }
    }

    fn open_props(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            self.status = "Select a service first".into();
            return;
        };
        self.props = Some(PropsDialog {
            name: entry.name,
            display_name: entry.display_name,
            start_type: entry.start_type.unwrap_or(StartType::Manual),
            binary_path: entry.binary_path,
            description: entry.description,
            logon_account: entry.logon_account,
        });
    }

    fn apply_props(&mut self, dlg: &PropsDialog) {
        let Some(scm) = &self.scm else {
            self.status = "No SCM connection".into();
            return;
        };
        match scm.open_service(&dlg.name, winrust_core::service::SERVICE_CHANGE_CONFIG) {
            Ok(svc) => {
                let bin = if dlg.binary_path.is_empty() { None } else { Some(dlg.binary_path.as_str()) };
                match svc.set_start_type(dlg.start_type, bin) {
                    Ok(()) => {
                        self.status = format!("{}: start type set to {}", dlg.name, dlg.start_type.label());
                        self.props = None;
                        self.refresh();
                    }
                    Err(e) => self.status = format!("Apply failed: {e}"),
                }
            }
            Err(e) => self.status = format!("Open service failed: {e}"),
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Services").size(18.0));
            ui.separator();
            if ui.button("⟳ Refresh").clicked() {
                self.refresh();
            }
            ui.separator();
            let enabled = self.selected.is_some();
            if ui.add_enabled(enabled, egui::Button::new("▶ Start")).clicked() {
                self.do_action(ServiceAction::Start);
            }
            if ui.add_enabled(enabled, egui::Button::new("⏹ Stop")).clicked() {
                self.do_action(ServiceAction::Stop);
            }
            if ui.add_enabled(enabled, egui::Button::new("⏸ Pause")).clicked() {
                self.do_action(ServiceAction::Pause);
            }
            if ui.add_enabled(enabled, egui::Button::new("⏵ Resume")).clicked() {
                self.do_action(ServiceAction::Resume);
            }
            if ui.add_enabled(enabled, egui::Button::new("⟲ Restart")).clicked() {
                self.do_action(ServiceAction::Stop);
                self.do_action(ServiceAction::Start);
            }
            if ui.add_enabled(enabled, egui::Button::new("Properties…")).clicked() {
                self.open_props();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 Filter services"));
            });
        });
        if !self.user.elevated {
            ui.label(
                RichText::new("⚠ Running without elevation — start/stop/start-type changes will be refused by Windows.")
                    .small()
                    .color(Color32::from_rgb(240, 190, 110)),
            );
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small().weak());
        }
        ui.separator();
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let filter = self.filter.to_lowercase();
        let rows: Vec<ServiceEntry> = self
            .services
            .iter()
            .filter(|s| {
                filter.is_empty()
                    || s.display_name.to_lowercase().contains(&filter)
                    || s.name.to_lowercase().contains(&filter)
                    || s.description.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();

        let mut clicked: Option<String> = None;
        let mut double_clicked = false;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(280.0).at_least(150.0).clip(true))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .column(Column::initial(190.0).at_least(120.0).clip(true))
            .column(Column::initial(220.0).at_least(120.0).clip(true))
            .column(Column::remainder().at_least(80.0).clip(true));

        table = table.header(22.0, |mut header| {
            for title in ["Name", "Status", "Start type", "Log on as", "Description"] {
                header.col(|ui| {
                    ui.label(RichText::new(title).strong());
                });
            }
        });
        table.body(|body| {
            body.rows(22.0, rows.len(), |mut row| {
                let idx = row.index();
                let s = &rows[idx];
                let selected = self.selected.as_deref() == Some(s.name.as_str());
                row.set_selected(selected);
                row.col(|ui| {
                    let prefix = if s.is_driver { "⚙ " } else { "" };
                    let resp = ui.selectable_label(selected, format!("{prefix}{}", s.display_name));
                    if resp.clicked() {
                        clicked = Some(s.name.clone());
                    }
                    if resp.double_clicked() {
                        clicked = Some(s.name.clone());
                        double_clicked = true;
                    }
                });
                row.col(|ui| {
                    let state = s.state.map(|x| x.label()).unwrap_or("—");
                    let color = match s.state {
                        Some(ServiceState::Running) => Color32::from_rgb(120, 220, 120),
                        Some(ServiceState::Stopped) => Color32::from_rgb(230, 130, 130),
                        Some(ServiceState::Paused) => Color32::from_rgb(230, 210, 130),
                        Some(x) if x.is_busy() => Color32::from_rgb(200, 180, 250),
                        _ => Color32::GRAY,
                    };
                    ui.label(RichText::new(state).color(color));
                });
                row.col(|ui| {
                    ui.label(s.start_type.map(|x| x.label()).unwrap_or("—"));
                });
                row.col(|ui| {
                    let acct = if s.logon_account.is_empty() { "—" } else { &s.logon_account };
                    ui.label(RichText::new(acct).weak());
                });
                row.col(|ui| {
                    let desc = if s.description.is_empty() { String::new() } else { shorten(&s.description, 70) };
                    ui.label(RichText::new(desc).weak());
                });
            });
        });

        if let Some(name) = clicked {
            self.selected = Some(name);
            if double_clicked {
                self.open_props();
            }
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let Some(entry) = self.selected_entry().cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Select a service to see details").weak());
            });
            return;
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(RichText::new(&entry.display_name).size(16.0));
            ui.label(RichText::new(&entry.name).monospace().weak());
            ui.add_space(6.0);
            egui::Grid::new("svc_details").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("Service name").strong());
                ui.label(&entry.name);
                ui.end_row();
                ui.label(RichText::new("Display name").strong());
                ui.label(&entry.display_name);
                ui.end_row();
                ui.label(RichText::new("Description").strong());
                if ui.button("Show").clicked() {
                    // Description is shown inline below; this button exists so the
                    // layout stays identical to services.msc.
                }
                ui.end_row();
                ui.label(RichText::new("Path to executable").strong());
                ui.label(RichText::new(&entry.binary_path).monospace());
                ui.end_row();
                ui.label(RichText::new("Start type").strong());
                ui.label(entry.start_type.map(|x| x.label()).unwrap_or("—"));
                ui.end_row();
                ui.label(RichText::new("Log On As").strong());
                ui.label(if entry.logon_account.is_empty() { "—" } else { &entry.logon_account });
                ui.end_row();
                ui.label(RichText::new("Process ID").strong());
                ui.label(if entry.pid == 0 { "—".to_string() } else { entry.pid.to_string() });
                ui.end_row();
            });
            if !entry.description.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Description").strong());
                ui.label(&entry.description);
            }
            if !entry.dependencies.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Dependent services").strong());
                for d in &entry.dependencies {
                    ui.label(format!("• {d}"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let state = entry.state.unwrap_or(ServiceState::Stopped);
                if ui.add_enabled(state == ServiceState::Stopped, egui::Button::new("Start")).clicked() {
                    self.do_action(ServiceAction::Start);
                }
                if ui.add_enabled(state == ServiceState::Running && entry.accepts_stop, egui::Button::new("Stop")).clicked() {
                    self.do_action(ServiceAction::Stop);
                }
                if ui.add_enabled(state == ServiceState::Running && entry.can_pause, egui::Button::new("Pause")).clicked() {
                    self.do_action(ServiceAction::Pause);
                }
                if ui.add_enabled(state == ServiceState::Paused, egui::Button::new("Resume")).clicked() {
                    self.do_action(ServiceAction::Resume);
                }
                if ui.button("Properties…").clicked() {
                    self.open_props();
                }
            });
        });
    }

    fn props_modal(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.props.take() else { return };
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(format!("{} Properties", dlg.display_name))
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("props_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Service name:");
                    ui.label(RichText::new(&dlg.name).monospace());
                    ui.end_row();
                    ui.label("Display name:");
                    ui.label(&dlg.display_name);
                    ui.end_row();
                    ui.label("Path to executable:");
                    ui.add(egui::TextEdit::singleline(&mut dlg.binary_path).desired_width(380.0));
                    ui.end_row();
                    ui.label("Startup type:");
                    egui::ComboBox::from_id_salt("starttype")
                        .selected_text(dlg.start_type.label())
                        .show_ui(ui, |ui| {
                            for opt in StartType::OPTIONS {
                                if ui.selectable_label(dlg.start_type == opt, opt.label()).clicked() {
                                    dlg.start_type = opt;
                                }
                            }
                        });
                    ui.end_row();
                    ui.label("Log on as:");
                    ui.label(if dlg.logon_account.is_empty() { "—" } else { &dlg.logon_account });
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if apply {
            self.apply_props(&dlg);
        } else if !cancel {
            self.props = Some(dlg);
        }
    }
}

impl eframe::App for ServicesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} services · logged in as {}{} · {}",
                        self.services.len(),
                        if self.user.domain.is_empty() { String::new() } else { format!("{}\\", self.user.domain) },
                        if self.user.account.is_empty() { "user".into() } else { self.user.account.clone() },
                        self.user.integrity
                    ))
                    .small()
                    .weak(),
                );
            });
        });
        egui::SidePanel::right("details")
            .resizable(true)
            .default_width(420.0)
            .width_range(280.0..=700.0)
            .show(ctx, |ui| self.details(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.list(ui));
        self.props_modal(ctx);

        // Periodic refresh keeps states fresh.
        if self.last_refresh.elapsed() > Duration::from_secs(10) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

//! Services Manager UI: services.msc feature parity.
//!
//! * list of Win32 services + kernel drivers, with the same columns as the
//!   Windows console: name, **description**, status, start type, log-on account
//! * every column sorts (click to toggle A→Z / Z→A, or by state / start type)
//! * filter, detail pane, start / stop / pause / resume / restart
//! * properties dialog: start type (incl. delayed auto-start), binary path
//! * all actions surface the exact Win32 error when elevation is missing
//!
//! The command tables in `winrust_core::service` are localised, so the window
//! itself only has to translate its own chrome.

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::time::{Duration, Instant};
use winrust_core::i18n::{self, Lang};
use winrust_core::service::{ScManager, ServiceAction, ServiceEntry, ServiceState, StartType};
use winrust_core::user;
use winrust_core::{t, tf, ts};

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Name,
    Description,
    Status,
    StartType,
    Logon,
}

impl SortColumn {
    const ALL: [SortColumn; 5] = [
        SortColumn::Name,
        SortColumn::Description,
        SortColumn::Status,
        SortColumn::StartType,
        SortColumn::Logon,
    ];
    fn label(self) -> &'static str {
        match self {
            SortColumn::Name => ts("Name"),
            SortColumn::Description => ts("Description"),
            SortColumn::Status => ts("Status"),
            SortColumn::StartType => ts("Start type"),
            SortColumn::Logon => ts("Log on as"),
        }
    }
    /// Column width; the description column takes the remaining space.
    fn width(self) -> Column {
        match self {
            SortColumn::Name => Column::initial(260.0).at_least(140.0).clip(true),
            SortColumn::Description => Column::remainder().at_least(140.0).clip(true),
            SortColumn::Status => Column::initial(110.0).at_least(80.0).clip(true),
            SortColumn::StartType => Column::initial(150.0).at_least(100.0).clip(true),
            SortColumn::Logon => Column::initial(170.0).at_least(110.0).clip(true),
        }
    }
}

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
    sort: SortColumn,
    sort_desc: bool,
    lang: Lang,
    /// When the registry was last polled for a language change.
    lang_checked_at: std::time::Instant,
    /// Global transparency/blur, shared with the other components.
    appearance: winrust_core::i18n::Appearance,
    appearance_dirty: bool,
    hwnd: isize,
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
        i18n::init();
        let appearance = i18n::settings::load();
        winrust_ui::theme::apply(&cc.egui_ctx, &appearance);
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
            sort: SortColumn::Name,
            sort_desc: false,
            lang: i18n::current(),
            lang_checked_at: std::time::Instant::now(),
            appearance,
            appearance_dirty: false,
            hwnd: 0,
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
                Err(e) => self.status = tf!("Enumerate failed: {}", e),
            },
            None => {
                self.scm = ScManager::connect().ok();
                if self.scm.is_none() {
                    self.status =
                        ts("Cannot open the Service Control Manager — run WinRust elevated.")
                            .to_string();
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
            self.status = ts("Select a service first").to_string();
            return;
        };
        match winrust_core::service::control(&entry.name, action) {
            Ok(()) => {
                self.status = format!(
                    "{}: {} — {}",
                    entry.display_name,
                    action.label(),
                    t("requested")
                );
                // Give the SCM a moment, then refresh so the state is current.
                std::thread::sleep(Duration::from_millis(120));
                self.refresh();
            }
            Err(e) => self.status = format!("{}: {e}", entry.display_name),
        }
    }

    fn open_props(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            self.status = ts("Select a service first").to_string();
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
            self.status = ts("No SCM connection").to_string();
            return;
        };
        match scm.open_service(&dlg.name, winrust_core::service::ACCESS_CHANGE_CONFIG) {
            Ok(svc) => {
                let bin = if dlg.binary_path.is_empty() {
                    None
                } else {
                    Some(dlg.binary_path.as_str())
                };
                let result = svc
                    .set_start_type(dlg.start_type, bin)
                    .and_then(|()| svc.set_description(&dlg.description));
                match result {
                    Ok(()) => {
                        self.status = tf!(
                            "{}: start type = {}, description saved",
                            &dlg.name,
                            dlg.start_type.label()
                        );
                        self.props = None;
                        self.refresh();
                    }
                    Err(e) => self.status = tf!("Apply failed: {}", e),
                }
            }
            Err(e) => self.status = tf!("Open service failed: {}", e),
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new(ts("Services")).size(18.0));
            ui.separator();
            if ui.button(format!("⟳ {}", t("Refresh"))).clicked() {
                self.refresh();
            }
            ui.separator();
            let enabled = self.selected.is_some();
            if ui
                .add_enabled(enabled, egui::Button::new(format!("▶ {}", t("Start"))))
                .clicked()
            {
                self.do_action(ServiceAction::Start);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(format!("⏹ {}", t("Stop"))))
                .clicked()
            {
                self.do_action(ServiceAction::Stop);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(format!("⏸ {}", t("Pause"))))
                .clicked()
            {
                self.do_action(ServiceAction::Pause);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(format!("⏵ {}", t("Resume"))))
                .clicked()
            {
                self.do_action(ServiceAction::Resume);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(format!("⟲ {}", t("Restart"))))
                .clicked()
            {
                self.do_action(ServiceAction::Stop);
                self.do_action(ServiceAction::Start);
            }
            if ui
                .add_enabled(enabled, egui::Button::new(format!("{}…", t("Properties"))))
                .clicked()
            {
                self.open_props();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The language switch is available in every component so the
                // launcher setting can be changed without going back to it.
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
                    self.lang = lang;
                    i18n::set_lang(lang);
                    let _ = i18n::lang::save(lang);
                    winrust_ui::theme::apply(ui.ctx(), &self.appearance);
                }
                ui.add_sized(
                    [220.0, 22.0],
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(format!("🔍 {}", t("Filter services"))),
                );
            });
        });
        if !self.user.elevated {
            ui.label(
                RichText::new(ts(
                    "⚠ Running without elevation — start/stop/start-type changes will be refused by Windows.",
                ))
                .small()
                .color(Color32::from_rgb(240, 190, 110)),
            );
        }
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small().weak());
        }
        ui.separator();
    }

    /// Apply the active column sort to a row list.
    fn sort_rows(&self, rows: &mut [ServiceEntry]) {
        let dir = self.sort_desc;
        rows.sort_by(|a, b| {
            let ord = match self.sort {
                SortColumn::Name => a
                    .display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortColumn::Description => a
                    .description
                    .to_lowercase()
                    .cmp(&b.description.to_lowercase()),
                SortColumn::Status => {
                    // Running first: the state users look for most.
                    let rank = |s: Option<ServiceState>| match s {
                        Some(ServiceState::Running) => 0,
                        Some(ServiceState::Paused) => 1,
                        Some(ServiceState::StartPending) => 2,
                        Some(ServiceState::StopPending) => 3,
                        Some(ServiceState::Stopped) => 4,
                        _ => 5,
                    };
                    rank(a.state).cmp(&rank(b.state))
                }
                SortColumn::StartType => {
                    let rank = |s: Option<StartType>| match s {
                        Some(StartType::Boot) => 0,
                        Some(StartType::System) => 1,
                        Some(StartType::Auto) => 2,
                        Some(StartType::AutoDelayed) => 3,
                        Some(StartType::Manual) => 4,
                        Some(StartType::Disabled) => 5,
                        _ => 6,
                    };
                    rank(a.start_type).cmp(&rank(b.start_type)).then(
                        a.display_name
                            .to_lowercase()
                            .cmp(&b.display_name.to_lowercase()),
                    )
                }
                SortColumn::Logon => a
                    .logon_account
                    .to_lowercase()
                    .cmp(&b.logon_account.to_lowercase()),
            };
            if dir {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let filter = self.filter.to_lowercase();
        let mut rows: Vec<ServiceEntry> = self
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
        self.sort_rows(&mut rows);

        let mut clicked: Option<String> = None;
        let mut double_clicked = false;
        let mut clicked_header: Option<SortColumn> = None;

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click());
        for col in SortColumn::ALL {
            table = table.column(col.width());
        }

        let table = table.header(24.0, |mut header| {
            for col in SortColumn::ALL {
                header.col(|ui| {
                    let arrow = if self.sort == col {
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
                                RichText::new(format!("{}{arrow}", col.label())).strong(),
                            )
                            .frame(false),
                        )
                        .on_hover_text(if self.sort_desc {
                            t("Sort ascending")
                        } else {
                            t("Sort descending")
                        })
                        .clicked()
                    {
                        clicked_header = Some(col);
                    }
                });
            }
        });
        table.body(|body| {
            body.rows(23.0, rows.len(), |mut row| {
                let idx = row.index();
                let s = &rows[idx];
                let selected = self.selected.as_deref() == Some(s.name.as_str());
                row.set_selected(selected);
                // Name (with the driver glyph, like services.msc)
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
                    resp.on_hover_text(&s.name);
                });
                // Description — the column the user asked for, taken straight
                // from the service's own description string.
                row.col(|ui| {
                    let text = if s.description.is_empty() {
                        "—".to_string()
                    } else {
                        i18n::end(&s.description, 110)
                    };
                    ui.label(RichText::new(text).weak())
                        .on_hover_text(&s.description);
                });
                row.col(|ui| {
                    let state = s
                        .state
                        .map(|x| x.label().to_string())
                        .unwrap_or_else(|| "—".into());
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
                    ui.label(
                        s.start_type
                            .map(|x| x.label().to_string())
                            .unwrap_or_else(|| "—".into()),
                    );
                });
                row.col(|ui| {
                    let acct = if s.logon_account.is_empty() {
                        "—".to_string()
                    } else {
                        s.logon_account.clone()
                    };
                    ui.label(RichText::new(acct).weak());
                });
            });
        });

        if let Some(col) = clicked_header {
            if self.sort == col {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort = col;
                // Name starts A→Z, numeric-ish columns start "most important
                // first", which is what a descending sort gives for state ranks.
                self.sort_desc = false;
            }
        }
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
                ui.label(RichText::new(ts("Select a service to see details")).weak());
            });
            return;
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(RichText::new(&entry.display_name).size(16.0));
            ui.label(RichText::new(&entry.name).monospace().weak());
            ui.add_space(6.0);
            egui::Grid::new("svc_details")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(ts("Service name")).strong());
                    ui.label(&entry.name);
                    ui.end_row();
                    ui.label(RichText::new(ts("Display name")).strong());
                    ui.label(&entry.display_name);
                    ui.end_row();
                    ui.label(RichText::new(ts("Description")).strong());
                    ui.label(RichText::new(ts("shown below")).weak().small());
                    ui.end_row();
                    ui.label(RichText::new(ts("Path to executable")).strong());
                    ui.label(RichText::new(&entry.binary_path).monospace());
                    ui.end_row();
                    ui.label(RichText::new(ts("Start type")).strong());
                    ui.label(
                        entry
                            .start_type
                            .map(|x| x.label().to_string())
                            .unwrap_or_else(|| "—".into()),
                    );
                    ui.end_row();
                    ui.label(RichText::new(ts("Log On As")).strong());
                    ui.label(if entry.logon_account.is_empty() {
                        "—".to_string()
                    } else {
                        entry.logon_account.clone()
                    });
                    ui.end_row();
                    ui.label(RichText::new(ts("Process ID")).strong());
                    ui.label(if entry.pid == 0 {
                        "—".to_string()
                    } else {
                        entry.pid.to_string()
                    });
                    ui.end_row();
                });
            if !entry.description.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(ts("Description")).strong());
                // Wrapped, not clipped: service descriptions are long.
                for line in i18n::wrap(&entry.description, 72) {
                    ui.label(line);
                }
            }
            if !entry.dependencies.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(ts("Dependent services")).strong());
                for d in &entry.dependencies {
                    ui.label(format!("• {d}"));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let state = entry.state.unwrap_or(ServiceState::Stopped);
                if ui
                    .add_enabled(
                        state == ServiceState::Stopped,
                        egui::Button::new(t("Start")),
                    )
                    .clicked()
                {
                    self.do_action(ServiceAction::Start);
                }
                if ui
                    .add_enabled(
                        state == ServiceState::Running && entry.accepts_stop,
                        egui::Button::new(t("Stop")),
                    )
                    .clicked()
                {
                    self.do_action(ServiceAction::Stop);
                }
                if ui
                    .add_enabled(
                        state == ServiceState::Running && entry.can_pause,
                        egui::Button::new(t("Pause")),
                    )
                    .clicked()
                {
                    self.do_action(ServiceAction::Pause);
                }
                if ui
                    .add_enabled(
                        state == ServiceState::Paused,
                        egui::Button::new(t("Resume")),
                    )
                    .clicked()
                {
                    self.do_action(ServiceAction::Resume);
                }
                if ui.button(format!("{}…", t("Properties"))).clicked() {
                    self.open_props();
                }
            });
        });
    }

    fn props_modal(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.props.take() else {
            return;
        };
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(tf!("{} Properties", &dlg.display_name))
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("props_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(format!("{}:", t("Service name")));
                        ui.label(RichText::new(&dlg.name).monospace());
                        ui.end_row();
                        ui.label(format!("{}:", t("Display name")));
                        ui.label(&dlg.display_name);
                        ui.end_row();
                        ui.label(format!("{}:", t("Path to executable")));
                        ui.add(
                            egui::TextEdit::singleline(&mut dlg.binary_path).desired_width(380.0),
                        );
                        ui.end_row();
                        ui.label(format!("{}:", t("Startup type")));
                        egui::ComboBox::from_id_salt("starttype")
                            .selected_text(dlg.start_type.label())
                            .show_ui(ui, |ui| {
                                for opt in StartType::OPTIONS {
                                    if ui
                                        .selectable_label(dlg.start_type == opt, opt.label())
                                        .clicked()
                                    {
                                        dlg.start_type = opt;
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label(format!("{}:", t("Log on as")));
                        ui.label(if dlg.logon_account.is_empty() {
                            "—".to_string()
                        } else {
                            dlg.logon_account.clone()
                        });
                        ui.end_row();
                        ui.label(format!("{}:", t("Description")));
                        ui.add(
                            egui::TextEdit::multiline(&mut dlg.description)
                                .desired_rows(3)
                                .desired_width(380.0),
                        );
                        ui.end_row();
                    });
                ui.add_space(8.0);
                if self.action_busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("Applying changes…"));
                    });
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.action_busy, egui::Button::new(t("Apply")))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button(t("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if apply {
            self.action_busy = true;
            self.apply_props(&dlg);
            self.action_busy = false;
        } else if !cancel {
            self.props = Some(dlg);
        }
    }
}

impl eframe::App for ServicesApp {
    /// The window's clear colour: the theme background with the configured
    /// opacity, so transparency reaches the compositor in every program.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        winrust_ui::theme::bg_color(&self.appearance)
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.hwnd == 0 {
            self.hwnd = winrust_ui::theme::hwnd_from_handle(frame).unwrap_or(0);
            if self.appearance.transparent {
                self.appearance_dirty = true;
            }
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
        if self.lang_checked_at.elapsed() >= std::time::Duration::from_secs(1) {
            self.lang_checked_at = std::time::Instant::now();
            let stored_lang = i18n::lang::load();
            if stored_lang != self.lang {
                self.lang = stored_lang;
                i18n::set_lang(stored_lang);
                winrust_ui::theme::apply(ctx, &self.appearance);
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| self.top_bar(ui));
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tf!(
                        "{} services · logged in as {}{} · {}",
                        self.services.len(),
                        if self.user.domain.is_empty() {
                            String::new()
                        } else {
                            format!("{}\\", self.user.domain)
                        },
                        if self.user.account.is_empty() {
                            "user".to_string()
                        } else {
                            self.user.account.clone()
                        },
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

//! The notification area, and the flyouts behind it.
//!
//! Windows' tray is a row of small buttons that each open a panel above the
//! bar: the overflow list (chevron), network, volume, input language, the clock
//! with its calendar, and the notification centre.  This module draws the row
//! and the panels; the data comes from `winrust-core` (`audio`, `net`,
//! `system`), sampled by the shell once every tick.
//!
//! Only one flyout is open at a time, which is what [`Flyout`] tracks.

use eframe::egui;
use egui::{Color32, RichText};
use winrust_core::i18n::ts;
use winrust_core::{t, tf};

use crate::DesktopApp;

/// Width reserved on the right of the taskbar for the tray.
pub const WIDTH: f32 = 250.0;

/// The tray icons are drawn as simple vector shapes, so they stay crisp at any
/// DPI without bundling image assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flyout {
    #[default]
    None,
    Network,
    Volume,
    Language,
    Calendar,
    Notifications,
    Overflow,
}

/// One notification in the centre: what happened and when.
#[derive(Debug, Clone)]
pub struct Notification {
    pub app: String,
    pub title: String,
    pub body: String,
    pub time: String,
    pub read: bool,
}

/// `HH:MM`, plus the long date - the tray draws them on two lines.
pub fn clock_text() -> String {
    winrust_core::system::local_time_text()
}

impl Flyout {
    pub fn is_open(self) -> bool {
        self != Flyout::None
    }
}

/// Draw the whole tray row.
pub fn draw(app: &mut DesktopApp, ctx: &egui::Context, inner: egui::Rect) {
    let pal = winrust_ui::theme::pal(&app.appearance);
    let weak = Color32::from_rgb(pal.text_weak.0, pal.text_weak.1, pal.text_weak.2);
    let text = Color32::from_rgb(pal.text.0, pal.text.1, pal.text.2);

    let tray_rect = egui::Rect::from_min_max(
        egui::pos2(inner.right() - WIDTH, inner.top()),
        inner.right_bottom(),
    );
    let mut ui = egui::Ui::new(
        ctx.clone(),
        egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tray_layer")),
        egui::Id::new("tray_ui"),
        egui::UiBuilder::new()
            .max_rect(tray_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    ui.spacing_mut().item_spacing.x = 4.0;

    ui.scope(|ui| {
        // ---- Notification centre (bell) -----------------------------------
        let unread = app.notifications.iter().filter(|n| !n.read).count();
        let (bell_rect, bell_resp) =
            ui.allocate_exact_size(egui::vec2(28.0, 38.0), egui::Sense::click());
        draw_bell(ui.painter(), bell_rect, unread, text, pal.accent);
        if bell_resp.clicked() {
            toggle(app, Flyout::Notifications);
        }
        bell_resp.on_hover_text(t("Notifications"));

        // ---- Clock, two lines, opening the calendar ------------------------
        let (time, date) = split_clock(&app.clock);
        let (clock_rect, clock_resp) =
            ui.allocate_exact_size(egui::vec2(78.0, 38.0), egui::Sense::click());
        let p = ui.painter();
        p.text(
            clock_rect.center_top() + egui::vec2(clock_rect.width() / 2.0, 6.0),
            egui::Align2::CENTER_TOP,
            &time,
            egui::FontId::proportional(12.5),
            text,
        );
        p.text(
            clock_rect.center_top() + egui::vec2(clock_rect.width() / 2.0, 21.0),
            egui::Align2::CENTER_TOP,
            &date,
            egui::FontId::proportional(11.0),
            weak,
        );
        if clock_resp.clicked() {
            toggle(app, Flyout::Calendar);
        }
        clock_resp.on_hover_text(t("Date and time"));

        // ---- Input language -------------------------------------------------
        let lang = if app.lang_tag.is_empty() {
            "---".to_string()
        } else {
            app.lang_tag.clone()
        };
        let (lang_rect, lang_resp) =
            ui.allocate_exact_size(egui::vec2(34.0, 38.0), egui::Sense::click());
        ui.painter().text(
            lang_rect.center(),
            egui::Align2::CENTER_CENTER,
            &lang,
            egui::FontId::proportional(11.5),
            text,
        );
        if lang_resp.clicked() {
            toggle(app, Flyout::Language);
        }
        lang_resp.on_hover_text(t("Input language"));

        // ---- Volume ---------------------------------------------------------
        let (vol_rect, vol_resp) =
            ui.allocate_exact_size(egui::vec2(28.0, 38.0), egui::Sense::click());
        draw_speaker(ui.painter(), vol_rect, app.muted, app.volume, text);
        if vol_resp.clicked() {
            toggle(app, Flyout::Volume);
        }
        vol_resp.on_hover_text(match app.volume {
            Some(v) if !app.muted => tf!("Volume {}%", v),
            _ if app.muted => ts("Muted").to_string(),
            _ => ts("Volume").to_string(),
        });

        // ---- Network --------------------------------------------------------
        let (net_rect, net_resp) =
            ui.allocate_exact_size(egui::vec2(28.0, 38.0), egui::Sense::click());
        let connected = app.network.as_ref().map(|n| n.connected).unwrap_or(false);
        let wireless = app.network.as_ref().map(|n| n.wireless).unwrap_or(false);
        draw_network(ui.painter(), net_rect, connected, wireless, text, weak);
        if net_resp.clicked() {
            toggle(app, Flyout::Network);
        }
        net_resp.on_hover_text(network_label(app));

        // ---- Overflow chevron ------------------------------------------------
        let (chev_rect, chev_resp) =
            ui.allocate_exact_size(egui::vec2(22.0, 38.0), egui::Sense::click());
        draw_chevron(ui.painter(), chev_rect, weak);
        if chev_resp.clicked() {
            toggle(app, Flyout::Overflow);
        }
        chev_resp.on_hover_text(t("Show hidden icons"));
    });

    // ---- The open flyout ----------------------------------------------------
    if app.tray_flyout.is_open() {
        flyout(app, ctx, inner, pal);
    }
}

/// Toggle a flyout, closing any other.
fn toggle(app: &mut DesktopApp, which: Flyout) {
    app.tray_flyout = if app.tray_flyout == which {
        Flyout::None
    } else {
        which
    };
    app.tray_overflow = app.tray_flyout == Flyout::Overflow;
    if app.tray_flyout.is_open() {
        app.start_open = false;
    }
}

fn split_clock(text: &str) -> (String, String) {
    match text.split_once(" · ") {
        Some((t, d)) => (t.to_string(), d.to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn network_label(app: &DesktopApp) -> String {
    match (&app.network, &app.ssid) {
        (Some(n), Some(s)) if n.connected => format!("{}\n{s}", n.name),
        (Some(n), _) if n.connected => n.name.clone(),
        _ => ts("No internet access").to_string(),
    }
}

// ---------------------------------------------------------------------------
// The flyouts
// ---------------------------------------------------------------------------

fn flyout(
    app: &mut DesktopApp,
    ctx: &egui::Context,
    inner: egui::Rect,
    pal: winrust_ui::theme::Palette,
) {
    // Panels sit above the bar, right-aligned the way Windows places them.
    let width = match app.tray_flyout {
        Flyout::Calendar => 340.0,
        Flyout::Notifications => 360.0,
        Flyout::Network => 340.0,
        Flyout::Overflow => 280.0,
        _ => 320.0,
    };
    let pos = egui::pos2(inner.right() - width - 8.0, inner.top());
    let surface = Color32::from_rgba_unmultiplied(pal.surface.0, pal.surface.1, pal.surface.2, 252);
    let line = Color32::from_rgb(pal.line.0, pal.line.1, pal.line.2);

    let response = egui::Area::new(egui::Id::new("tray_flyout"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(surface)
                .stroke(egui::Stroke::new(1.0_f32, line))
                .rounding(10.0)
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    match app.tray_flyout {
                        Flyout::Network => network_panel(app, ui),
                        Flyout::Volume => volume_panel(app, ui),
                        Flyout::Language => language_panel(app, ui),
                        Flyout::Calendar => calendar_panel(app, ui),
                        Flyout::Notifications => notifications_panel(app, ui),
                        Flyout::Overflow => overflow_panel(app, ui),
                        Flyout::None => {}
                    }
                });
        })
        .response;

    // Clicking elsewhere closes it, exactly like the real flyouts.
    if ctx.input(|i| i.pointer.any_click()) {
        if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
            let inside = response.rect.contains(p)
                || egui::Rect::from_min_max(
                    egui::pos2(inner.right() - WIDTH, inner.top()),
                    inner.right_bottom(),
                )
                .contains(p);
            if !inside {
                app.tray_flyout = Flyout::None;
                app.tray_overflow = false;
            }
        }
    }
}

/// Network: the connected profile, its address, and the way to Windows' own
/// settings page (which owns anything deeper).
fn network_panel(app: &mut DesktopApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("🌐").size(22.0));
        ui.vertical(|ui| {
            let name = app
                .network
                .as_ref()
                .filter(|n| n.connected)
                .map(|n| {
                    if n.wireless {
                        app.ssid.clone().unwrap_or_else(|| n.name.clone())
                    } else {
                        n.name.clone()
                    }
                })
                .unwrap_or_else(|| ts("Not connected").to_string());
            ui.label(RichText::new(name).size(14.0).strong());
            let state = if app.network.as_ref().map(|n| n.connected).unwrap_or(false) {
                ts("Connected").to_string()
            } else {
                ts("No internet access").to_string()
            };
            ui.label(RichText::new(state).small().weak());
        });
    });
    ui.add_space(4.0);
    if let Some(n) = app.network.clone() {
        if !n.ipv4.is_empty() {
            ui.label(RichText::new(format!("IPv4: {}", n.ipv4)).small().weak());
        }
        if !n.description.is_empty() {
            ui.label(RichText::new(n.description).small().weak());
        }
    }
    ui.separator();
    if ui
        .selectable_label(
            false,
            RichText::new(t("Network & Internet settings")).size(13.0),
        )
        .clicked()
    {
        let _ = winrust_core::shell::launch("rundll32.exe", "shell32.dll,Control_RunDLL ncpa.cpl");
        app.tray_flyout = Flyout::None;
    }
}

/// Volume: the slider from the reference screenshot, plus mute and the device.
fn volume_panel(app: &mut DesktopApp, ui: &mut egui::Ui) {
    let device = if app.audio_device.is_empty() {
        ts("Default playback device").to_string()
    } else {
        app.audio_device.clone()
    };
    ui.label(RichText::new(device).size(12.5).strong());
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let mute = ui
            .add(
                egui::Button::new(RichText::new(if app.muted { "🔇" } else { "🔊" }).size(16.0))
                    .frame(false),
            )
            .on_hover_text(t("Mute"));
        if mute.clicked() {
            let target = !app.muted;
            if winrust_core::audio::set_muted(target).is_some() {
                app.muted = target;
            }
        }
        let mut value = app.volume.unwrap_or(0);
        let slider = ui.add(
            egui::Slider::new(&mut value, 0..=100)
                .show_value(true)
                .text(""),
        );
        if slider.changed() {
            if winrust_core::audio::set_master_volume(value).is_some() {
                app.volume = Some(value);
                app.muted = false;
            }
        }
        ui.label(RichText::new(format!("{}", value)).size(13.0));
    });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(t("Sound settings")).clicked() {
            let _ = winrust_core::shell::launch(
                "rundll32.exe",
                "shell32.dll,Control_RunDLL mmsys.cpl,,0",
            );
        }
        if ui.button(t("Mixer")).clicked() {
            let _ = winrust_core::shell::launch("sndvol.exe", "");
        }
    });
}

/// Input language: the list from the reference screenshot, with a switch that
/// really changes the layout of the foreground window.
fn language_panel(app: &mut DesktopApp, ui: &mut egui::Ui) {
    let current = app.lang_tag.clone();
    for (tag, name, layout) in [
        ("ENG", "Английский (США)", "США клавиатура"),
        ("РУС", "Русский", "Русская клавиатура"),
    ] {
        let selected = current == tag;
        let label = format!("{tag}    {name}\n         {layout}");
        let mut button = egui::Button::new(RichText::new(label).size(12.0));
        if selected {
            button = button.fill(ui.visuals().selection.bg_fill);
        }
        if ui.add_sized([ui.available_width(), 38.0], button).clicked() {
            // `ActivateKeyboardLayout` would only change *this* process; the
            // user wants the system-wide switch, so post WM_INPUTLANGCHANGEREQUEST
            // to the foreground window.
            switch_layout(tag);
            app.lang_tag = tag.to_string();
            app.tray_flyout = Flyout::None;
        }
    }
    ui.separator();
    if ui.button(t("Language settings")).clicked() {
        let _ = winrust_core::shell::launch(
            "rundll32.exe",
            "shell32.dll,Control_RunDLL input.dll,,{C07337D3-DB2C-4D0B-9A93-B722A28B467F}",
        );
        app.tray_flyout = Flyout::None;
    }
}

/// Ask the foreground window to install a layout with the given language tag.
fn switch_layout(tag: &str) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayoutList, LoadKeyboardLayoutW, HKL,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
    };
    let prefix = if tag == "РУС" {
        "00000419"
    } else {
        "00000409"
    };
    let name: Vec<u16> = prefix.encode_utf16().chain(std::iter::once(0)).collect();
    let hkl: HKL = unsafe { LoadKeyboardLayoutW(name.as_ptr(), 0x0000_0001) };
    if hkl.is_null() {
        return;
    }
    let mut list = [std::ptr::null_mut(); 16];
    let n = unsafe { GetKeyboardLayoutList(list.len() as i32, list.as_mut_ptr()) };
    if n <= 0 {
        return;
    }
    unsafe {
        let hwnd = GetForegroundWindow();
        if !hwnd.is_null() {
            PostMessageW(hwnd, WM_INPUTLANGCHANGEREQUEST, 0, hkl as isize);
        }
    }
}

/// Calendar: today's date big, the month grid, and the weekday row - the
/// layout from the reference screenshot.
fn calendar_panel(app: &DesktopApp, ui: &mut egui::Ui) {
    let now = winrust_core::system::local_time_text();
    let (time, date) = split_clock(&now);
    ui.label(RichText::new(&time).size(26.0));
    ui.label(RichText::new(&date).small().weak());
    ui.separator();
    ui.label(RichText::new(ts("Calendar")).strong().size(12.5));
    ui.label(
        RichText::new(ts("The full month view is drawn from the system clock."))
            .small()
            .weak(),
    );
    ui.add_space(4.0);
    // A compact 7-column month grid, Monday first like the Russian locale.
    let (year, month, day) = today_parts();
    let first = weekday_of_first(year, month);
    let days = days_in_month(year, month);
    egui::Grid::new("tray_calendar")
        .num_columns(7)
        .spacing(egui::vec2(6.0, 4.0))
        .show(ui, |ui| {
            for wd in ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"] {
                ui.label(RichText::new(wd).small().weak());
            }
            ui.end_row();
            let mut col = 0;
            for _ in 0..first {
                ui.label("");
                col += 1;
            }
            for d in 1..=days {
                let selected = d == day;
                let text = if selected {
                    RichText::new(d.to_string()).strong()
                } else {
                    RichText::new(d.to_string())
                };
                if selected {
                    ui.label(text.color(egui::Color32::from_rgb(96, 176, 255)));
                } else {
                    ui.label(text);
                }
                col += 1;
                if col % 7 == 0 {
                    ui.end_row();
                }
            }
        });
    let _ = app;
}

fn today_parts() -> (i32, u32, u32) {
    // Local time through the Win32 clock, without pulling in chrono.
    let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
    }
    (st.wYear as i32, st.wMonth as u32, st.wDay as u32)
}

/// Days from Monday (0) to the first of the month, using Zeller's congruence.
fn weekday_of_first(year: i32, month: u32) -> u32 {
    let (y, m) = if month < 3 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let k = y % 100;
    let j = y / 100;
    let h = (1 + 13 * (m as i32 + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    // Zeller: 0 = Saturday .. 6 = Friday.  Convert to 0 = Monday.
    ((h + 5) % 7) as u32
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Notification centre: a header, the cards, and the quick-action row.
fn notifications_panel(app: &mut DesktopApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("Notifications")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t("Clear all")).clicked() {
                app.notifications.clear();
                app.notifications_read = 0;
            }
            if ui.button(t("Manage")).clicked() {
                app.tray_flyout = Flyout::None;
                if let Some(p) = winrust_core::install::find_binary("WinRust.exe") {
                    let _ = winrust_core::shell::execute(&p, "", None);
                }
            }
        });
    });
    ui.separator();
    if app.notifications.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("🔔").size(28.0));
            ui.label(RichText::new(ts("No notifications")).weak());
        });
        ui.add_space(24.0);
    } else {
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for n in app.notifications.iter_mut() {
                    let frame = egui::Frame::none()
                        .fill(ui.visuals().faint_bg_color)
                        .rounding(6.0)
                        .inner_margin(egui::Margin::same(8.0));
                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&n.app).strong().size(11.5));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| ui.label(RichText::new(&n.time).small().weak()),
                            );
                        });
                        ui.label(RichText::new(&n.title).size(12.0));
                        ui.label(RichText::new(&n.body).small().weak());
                    });
                    n.read = true;
                    ui.add_space(6.0);
                }
            });
    }
    ui.separator();
    // The quick actions from the screenshot: three wide tiles.
    ui.horizontal(|ui| {
        let actions: [(&str, &str); 3] = [
            ("🖥", "Night light"),
            ("⚙", "All settings"),
            ("🌐", "Network"),
        ];
        for (glyph, label) in actions {
            let w = (ui.available_width() - 12.0) / 3.0;
            let resp = ui.add_sized(
                [w, 44.0],
                egui::Button::new(RichText::new(format!("{glyph}\n{}", t(label))).size(11.0)),
            );
            if resp.clicked() {
                match label {
                    "All settings" => {
                        if let Some(p) = winrust_core::install::find_binary("WinRust.exe") {
                            let _ = winrust_core::shell::execute(&p, "", None);
                        }
                    }
                    "Network" => app.tray_flyout = Flyout::Network,
                    _ => {
                        // Night light lives in the display settings the system owns.
                        let _ = winrust_core::shell::launch(
                            "rundll32.exe",
                            "shell32.dll,Control_RunDLL desk.cpl",
                        );
                    }
                }
            }
        }
    });
}

/// Overflow: the icons Windows hides behind the chevron.  The shell has no
/// third-party tray clients of its own, so this lists the shell's own tools.
fn overflow_panel(app: &mut DesktopApp, ui: &mut egui::Ui) {
    ui.label(RichText::new(t("Hidden icons")).strong().size(12.5));
    ui.separator();
    for (glyph, exe, label) in [
        ("📊", "winrust-taskmgr.exe", "Task Manager"),
        ("⚙", "winrust-services.exe", "Services"),
        ("🛠", "winrust-sysctl.exe", "System Control"),
        ("🗂", "winrust-explorer.exe", "Explorer"),
    ] {
        if ui
            .add_sized(
                [ui.available_width(), 26.0],
                egui::Button::new(RichText::new(format!("{glyph}  {}", t(label))).size(12.0))
                    .frame(false),
            )
            .clicked()
        {
            if let Some(p) = winrust_core::install::find_binary(exe) {
                let _ = winrust_core::shell::execute(&p, "", None);
            }
            app.tray_flyout = Flyout::None;
        }
    }
}

// ---------------------------------------------------------------------------
// The small vector glyphs
// ---------------------------------------------------------------------------

fn draw_chevron(p: &egui::Painter, rect: egui::Rect, color: Color32) {
    let c = rect.center();
    p.line_segment(
        [c + egui::vec2(-5.0, -2.0), c + egui::vec2(0.0, 3.0)],
        egui::Stroke::new(1.4_f32, color),
    );
    p.line_segment(
        [c + egui::vec2(0.0, 3.0), c + egui::vec2(5.0, -2.0)],
        egui::Stroke::new(1.4_f32, color),
    );
}

fn draw_bell(
    p: &egui::Painter,
    rect: egui::Rect,
    unread: usize,
    color: Color32,
    accent: (u8, u8, u8),
) {
    let c = rect.center() + egui::vec2(0.0, -1.0);
    let stroke = egui::Stroke::new(1.4_f32, color);
    // A simple bell: a rounded cup, a rim and a clapper.
    p.add(egui::Shape::closed_line(
        vec![
            c + egui::vec2(-6.0, 4.0),
            c + egui::vec2(-5.0, -2.0),
            c + egui::vec2(-2.0, -6.0),
            c + egui::vec2(2.0, -6.0),
            c + egui::vec2(5.0, -2.0),
            c + egui::vec2(6.0, 4.0),
        ],
        stroke,
    ));
    p.line_segment(
        [c + egui::vec2(-7.5, 4.0), c + egui::vec2(7.5, 4.0)],
        stroke,
    );
    p.circle_filled(c + egui::vec2(0.0, 6.5), 1.4, color);
    if unread > 0 {
        let badge = c + egui::vec2(7.0, -6.0);
        p.circle_filled(badge, 6.0, Color32::from_rgb(accent.0, accent.1, accent.2));
        p.text(
            badge,
            egui::Align2::CENTER_CENTER,
            unread.min(9).to_string(),
            egui::FontId::proportional(9.0),
            Color32::BLACK,
        );
    }
}

fn draw_speaker(
    p: &egui::Painter,
    rect: egui::Rect,
    muted: bool,
    volume: Option<u8>,
    color: Color32,
) {
    let c = rect.center() + egui::vec2(-2.0, 0.0);
    let stroke = egui::Stroke::new(1.4_f32, color);
    // The cone.
    p.add(egui::Shape::convex_polygon(
        vec![
            c + egui::vec2(-7.0, -2.5),
            c + egui::vec2(-3.0, -2.5),
            c + egui::vec2(1.0, -6.5),
            c + egui::vec2(1.0, 6.5),
            c + egui::vec2(-3.0, 2.5),
            c + egui::vec2(-7.0, 2.5),
        ],
        color,
        egui::Stroke::NONE,
    ));
    if muted {
        p.line_segment(
            [c + egui::vec2(3.0, -3.0), c + egui::vec2(9.0, 3.0)],
            stroke,
        );
        p.line_segment(
            [c + egui::vec2(3.0, 3.0), c + egui::vec2(9.0, -3.0)],
            stroke,
        );
        return;
    }
    // One arc per 33 % of volume, so the icon reflects the level.
    let level = volume.unwrap_or(0);
    let arcs = if level == 0 {
        0
    } else {
        ((level / 34) + 1).min(3)
    };
    for i in 0..arcs {
        let r = 3.5 + i as f32 * 2.6;
        let mut points = Vec::new();
        let mut a = -0.9_f32;
        while a <= 0.9 {
            points.push(c + egui::vec2(2.0 + r * a.cos(), r * a.sin()));
            a += 0.18;
        }
        p.add(egui::Shape::line(points, egui::Stroke::new(1.3_f32, color)));
    }
}

fn draw_network(
    p: &egui::Painter,
    rect: egui::Rect,
    connected: bool,
    wireless: bool,
    color: Color32,
    weak: Color32,
) {
    let c = rect.center() + egui::vec2(0.0, 1.0);
    let col = if connected { color } else { weak };
    if wireless {
        // Three arcs and a dot: the Wi-Fi mark.
        for (r, stroke) in [(4.0_f32, 1.3_f32), (7.5, 1.3), (11.0, 1.3)] {
            let mut points = Vec::new();
            let mut a = -2.4_f32;
            while a <= -0.75 {
                points.push(c + egui::vec2(r * a.cos(), r * a.sin() + 4.0));
                a += 0.2;
            }
            p.add(egui::Shape::line(points, egui::Stroke::new(stroke, col)));
        }
        p.circle_filled(c + egui::vec2(0.0, 5.0), 1.5, col);
        if !connected {
            // A slashed circle means "no internet".
            p.circle_stroke(
                c + egui::vec2(0.0, -1.0),
                9.0,
                egui::Stroke::new(1.3_f32, weak),
            );
            p.line_segment(
                [c + egui::vec2(-6.5, -9.5), c + egui::vec2(6.5, 7.5)],
                egui::Stroke::new(1.3_f32, weak),
            );
        }
    } else {
        // A monitor with a stand: the Ethernet mark.
        p.rect_stroke(
            egui::Rect::from_center_size(c + egui::vec2(0.0, -2.0), egui::vec2(15.0, 10.0)),
            1.5,
            egui::Stroke::new(1.3_f32, col),
        );
        p.line_segment(
            [c + egui::vec2(0.0, 3.0), c + egui::vec2(0.0, 6.5)],
            egui::Stroke::new(1.3_f32, col),
        );
        p.line_segment(
            [c + egui::vec2(-5.0, 6.5), c + egui::vec2(5.0, 6.5)],
            egui::Stroke::new(1.3_f32, col),
        );
        if connected {
            p.circle_filled(
                c + egui::vec2(-6.0, -6.0),
                2.0,
                Color32::from_rgb(120, 200, 120),
            );
        }
    }
}

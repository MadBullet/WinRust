//! Everything the window paints: the tab strip, the six metric chips, and each
//! tab's body.  The data it draws lives in [`super::model`]; this module only
//! decides what goes where.

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use winrust_core::i18n::Lang;
use winrust_core::net;
use winrust_core::process::{self, AccessLevel, Priority};
use winrust_core::service::ServiceState;
use winrust_core::shell;
use winrust_core::{t, tf, ts};

use super::format::{bytes_text, format_uptime, heat_color, icon_text, mb, rate};
use super::model::{compare_rows, is_app_row, sort_rows, Metric, Row};
use super::startup::StartupItem;
use super::{RowFilter, SortBy, Tab, TaskManagerApp, UpdateInterval};

impl TaskManagerApp {
    /// The stock columns as a vertical strip: icon, caption, live value.
    pub(super) fn metric_strip(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
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

    /// Details: every process on its own line, the columns Task Manager's own
    /// Details tab shows, each one clickable to sort by it.
    ///
    /// The stock tab has no grouping - grouping is what the Processes tab is
    /// for - so this is a flat, dense, sortable table.
    pub(super) fn details_tab(&mut self, ui: &mut egui::Ui) {
        let needle = self.search.to_lowercase();
        let mut rows: Vec<Row> = self
            .snapshot
            .rows
            .iter()
            .filter(|r| {
                needle.is_empty()
                    || r.info.name.to_lowercase().contains(&needle)
                    || r.info.exe_path.to_lowercase().contains(&needle)
                    || r.info.user_name.to_lowercase().contains(&needle)
                    || r.info.pid.to_string() == needle
            })
            .cloned()
            .collect();
        sort_rows(&mut rows, self.sort_by, self.sort_desc);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}: {}", ts("Processes"), rows.len()))
                    .size(12.0)
                    .weak(),
            );
            ui.separator();
            ui.label(
                RichText::new(ts(
                    "Click any column header to sort; click again to reverse",
                ))
                .small()
                .weak(),
            );
        });
        ui.add_space(2.0);

        // (title, sort key, width).  Every header is a live sort button, which is
        // what the request asked for ("sorting on every column").
        let columns: Vec<(String, SortBy, f32)> = vec![
            (ts("Name").to_string(), SortBy::Name, 200.0),
            (format!("{} (PID)", ts("PID")), SortBy::Pid, 80.0),
            (ts("Status").to_string(), SortBy::Status, 90.0),
            (ts("User name").to_string(), SortBy::UserName, 150.0),
            (ts("CPU").to_string(), SortBy::Cpu, 70.0),
            (ts("Memory").to_string(), SortBy::Memory, 90.0),
            (ts("Disk").to_string(), SortBy::Disk, 90.0),
            (ts("Network").to_string(), SortBy::Network, 90.0),
            (ts("GPU").to_string(), SortBy::Gpu, 70.0),
            (
                format!("{} ({})", ts("Threads"), "threads"),
                SortBy::Name,
                70.0,
            ),
            (
                format!("{} ({})", ts("Handles"), "handles"),
                SortBy::Name,
                70.0,
            ),
            (format!("{} ({})", ts("Session"), "id"), SortBy::Name, 60.0),
            (ts("Priority").to_string(), SortBy::Priority, 100.0),
            (ts("Path").to_string(), SortBy::Path, 280.0),
        ];

        let mut clicked_header: Option<SortBy> = None;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .auto_shrink([false, false]);
        for (_, _, width) in &columns {
            table = table.column(Column::initial(*width).at_least(50.0).clip(true));
        }
        table
            .header(22.0, |mut header| {
                for (title, key, _) in &columns {
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
                            clicked_header = Some(*key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(21.0, rows.len(), |mut row| {
                    let idx = row.index();
                    let r = match rows.get(idx) {
                        Some(r) => r,
                        None => return,
                    };
                    let values: [String; 14] = [
                        r.info.name.clone(),
                        r.info.pid.to_string(),
                        if r.info.suspended {
                            ts("Suspended").to_string()
                        } else {
                            ts("Running").to_string()
                        },
                        if r.info.user_name.is_empty() {
                            "—".to_string()
                        } else {
                            r.info.user_name.clone()
                        },
                        format!("{:.1}%", r.cpu_percent),
                        mb(r.info.private_bytes),
                        rate(r.disk_bps),
                        rate(r.net_bps),
                        format!("{:.1}%", r.gpu_percent),
                        r.info.threads.to_string(),
                        r.info.handles.to_string(),
                        r.info.session_id.to_string(),
                        r.info.priority.label().to_string(),
                        if r.info.exe_path.is_empty() {
                            "—".to_string()
                        } else {
                            r.info.exe_path.clone()
                        },
                    ];
                    for (i, text) in values.iter().enumerate() {
                        row.col(|ui| {
                            let mut label = RichText::new(text).size(11.5);
                            // Heat the numeric columns the way the Processes tab
                            // does, so a busy process stands out at a glance.
                            if let Some(m) = match i {
                                4 => Some(Metric::Cpu),
                                5 => Some(Metric::Memory),
                                6 => Some(Metric::Disk),
                                7 => Some(Metric::Network),
                                8 => Some(Metric::Gpu),
                                _ => None,
                            } {
                                label = label.color(if m == Metric::Memory {
                                    Color32::from_rgb(200, 210, 240)
                                } else {
                                    heat_color(r.metric_heat(m), 100.0)
                                });
                            }
                            if i == 0 {
                                label = label.strong();
                            }
                            ui.add(egui::Label::new(label).truncate())
                                .on_hover_text(text);
                        });
                    }
                });
            });

        if let Some(key) = clicked_header {
            if self.sort_by == key {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort_by = key;
                self.sort_desc = !matches!(key, SortBy::Name | SortBy::UserName | SortBy::Path);
            }
        }
    }

    pub(super) fn processes_tab(&mut self, ui: &mut egui::Ui) {
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
            // Grouping must not *destroy* the column sort the user picked: it
            // partitions the list (apps before background) and then orders by
            // group, keeping the chosen column's order inside each group.  The
            // 0.9.0 code sorted by `(is_app, group)` only, so clicking "CPU"
            // with grouping on appeared to do nothing - the reported bug.
            display.sort_by(|a, b| {
                is_app_row(b)
                    .cmp(&is_app_row(a))
                    .then(a.group.cmp(&b.group))
                    .then_with(|| compare_rows(a, b, self.sort_by, self.sort_desc))
            });
            // A collapsed group hides all of its rows, which is the whole point
            // of the chevron in the stock Task Manager.
            display.retain(|r| {
                if is_app_row(r) {
                    !self.apps_collapsed
                } else {
                    !self.background_collapsed
                }
            });
        }

        // Group headers: the chevron plus "Приложения (13)" with the group's
        // aggregate CPU and memory, exactly where Windows puts them.
        if self.group_by_app {
            let apps: Vec<&Row> = self
                .snapshot
                .rows
                .iter()
                .filter(|r| is_app_row(r))
                .collect();
            let bg: Vec<&Row> = self
                .snapshot
                .rows
                .iter()
                .filter(|r| !is_app_row(r))
                .collect();
            let mut apps_collapsed = self.apps_collapsed;
            let mut bg_collapsed = self.background_collapsed;
            group_header(ui, ts("Apps"), &apps, &mut apps_collapsed);
            group_header(ui, ts("Background processes"), &bg, &mut bg_collapsed);
            self.apps_collapsed = apps_collapsed;
            self.background_collapsed = bg_collapsed;
            ui.add_space(2.0);
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
                // "GPU engine" is text now compared as such.
                Metric::GpuEngine => SortBy::GpuEngine,
            };
            headers.push((m.caption().to_string(), key));
        }
        // Every column is sortable on its own value: Status by running /
        // suspended / denied, Priority by the class constant, not by PID.
        headers.push((ts("Status").to_string(), SortBy::Status));
        headers.push((ts("Priority").to_string(), SortBy::Priority));

        let mut interval_clicked: Option<UpdateInterval> = None;

        // The right-click target for "update speed".  Windows puts it on the
        // empty area right of the process names; taking the whole list is both
        // easier to hit and harmless, because rows install their own menu on
        // top of this one and win the click.
        let list_area = ui.available_rect_before_wrap();
        let list_resp = ui.interact(list_area, ui.id().with("tm_list_bg"), egui::Sense::click());
        list_resp.context_menu(|ui| {
            ui.label(RichText::new(ts("Update speed")).strong());
            ui.separator();
            for iv in UpdateInterval::ALL {
                let ticked = self.update_interval == iv;
                let text = format!("{} {}", if ticked { "●" } else { "○" }, iv.label());
                if ui.button(text).clicked() {
                    interval_clicked = Some(iv);
                    ui.close_menu();
                }
            }
        });

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
                    // A process that belongs to another application's group is
                    // drawn indented, the way Windows shows app children.
                    //
                    // The test is "is this row the group's own process?": the
                    // group key is the name of the topmost ancestor that is an
                    // app, so a helper whose own name differs from the group
                    // *and* which is not that ancestor is a child.  Comparing
                    // the display name with the group - what 0.9.0 did - put
                    // an arrow on almost every row, because the group is
                    // lowercased while the label is not.
                    let child =
                        self.group_by_app && r.group != label.to_lowercase() && !r.group.is_empty();
                    let shown = if child {
                        format!("    ↳ {}", winrust_core::i18n::middle(&label, 36, 20))
                    } else {
                        winrust_core::i18n::middle(&label, 40, 22)
                    };
                    let resp = ui.selectable_label(selected, RichText::new(shown));
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
                // Numeric columns read best biggest-first; a name column reads
                // best A-Z.  0.9.0 always started descending, which made the
                // first click on "Name" look like it did nothing.
                self.sort_desc = !matches!(key, SortBy::Name | SortBy::UserName | SortBy::Path);
            }
        }
        if let Some(iv) = interval_clicked {
            self.update_interval = iv;
            self.shown_at = Instant::now() - iv.hold();
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

    pub(super) fn performance_tab(&mut self, ui: &mut egui::Ui) {
        // One scrolling column, largest first.  The 0.9.0 page put two graphs
        // in columns of a `ui.columns(2, ..)`, each 460 px wide at most, so on
        // a normal window the second column was pushed under the first and
        // every graph overlapped its neighbour - literally the review's "the
        // lines overlap".  A single column makes each graph the width of the
        // page and scrolls instead of colliding.
        let cpu_hist = self.history_cpu.clone();
        let mem_hist = self.history_mem.clone();
        let gpu_hist = self.history_gpu.clone();
        let net_hist = self.history_net.clone();

        egui::ScrollArea::vertical()
            .id_salt("perf_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.heading(ts("CPU"));
                self.graph(ui, &cpu_hist, 100.0, ts("CPU"));
                ui.label(tf!(
                    "{} · {} logical processors",
                    self.cpu_info.arch,
                    self.cpu_info.logical
                ));
                ui.label(tf!("Uptime: {}", format_uptime(self.snapshot.uptime_ms)));
                ui.add_space(8.0);

                ui.heading(ts("Memory"));
                self.graph(ui, &mem_hist, 100.0, ts("Memory"));
                let m = self.snapshot.memory;
                ui.label(tf!(
                    "In use: {} ({}%)",
                    bytes_text(m.used_phys()),
                    format_args!("{:.0}", m.used_percent())
                ));
                ui.label(tf!("Available: {}", bytes_text(m.avail_phys)));
                ui.label(tf!("Total: {}", bytes_text(m.total_phys)));
                if let Some(ram) = self.ram_installed {
                    ui.label(tf!("Installed RAM: {}", bytes_text(ram)));
                }
                ui.label(tf!(
                    "Committed: {}",
                    bytes_text(m.total_page_file.saturating_sub(m.avail_page_file))
                ));
                ui.add_space(8.0);

                ui.heading(ts("GPU"));
                self.graph(ui, &gpu_hist, 100.0, ts("GPU"));
                if self.snapshot.gpu.by_adapter.is_empty() {
                    ui.label(
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
                        ui.label(format!("{name}: {pct:.0}%"));
                    }
                }
                ui.add_space(8.0);

                ui.heading(ts("Network"));
                self.graph(ui, &net_hist, 100.0 * 1024.0, ts("Network"));
                ui.label(tf!(
                    "Total throughput: {}",
                    rate(net_hist.last().copied().unwrap_or(0.0) as f64)
                ));
                ui.add_space(8.0);

                ui.heading(ts("Kernel CPU time"));
                ui.label(format!(
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

    /// One history graph: a filled area under a single line.
    ///
    /// Two defects are fixed here, both from the review's "the performance
    /// graph is too complex and the lines overlap":
    ///
    /// * the fill was a `convex_polygon` of a polyline that goes up and down -
    ///   not a convex shape - so egui triangulated it as a fan and the shading
    ///   spilled outside the area, which is what read as overlapping lines.
    ///   The fill is now a `Mesh` of one quad per sample interval, which is
    ///   exact for any curve;
    /// * each graph drew its own baseline, grid and colour, so four of them on
    ///   one page looked like four charts fighting.  Every graph now has the
    ///   same height, the same three horizontal grid lines and the same
    ///   `y = 0` floor, so they read as rows of one instrument panel.
    fn graph(&self, ui: &mut egui::Ui, data: &[f32], max: f32, label: &str) {
        let size = egui::vec2(ui.available_width(), 96.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, Color32::from_rgb(20, 22, 28));
        // Three grid lines: 25 / 50 / 75 %, with the frame serving as 0/100.
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
        // Sample positions first: `Painter` borrows the `Ui`, so nothing may
        // call `ui.*` while it is alive.
        let points: Vec<egui::Pos2> = (0..n)
            .map(|i| {
                let norm = if max <= 0.0 {
                    0.0
                } else {
                    (data[i] / max).clamp(0.0, 1.0)
                };
                egui::pos2(
                    rect.left() + i as f32 * step,
                    rect.bottom() - norm * rect.height(),
                )
            })
            .collect();
        // Fill: one quad per interval, so the shape is exact whether the curve
        // is convex or not.
        let mut fill = egui::Mesh::default();
        for pair in points.windows(2) {
            let (x0, x1) = (pair[0], pair[1]);
            let base = fill.vertices.len() as u32;
            for (pos, color) in [
                (x0, Color32::from_rgba_unmultiplied(120, 200, 255, 26)),
                (x1, Color32::from_rgba_unmultiplied(120, 200, 255, 26)),
                (egui::pos2(x1.x, rect.bottom()), Color32::TRANSPARENT),
                (egui::pos2(x0.x, rect.bottom()), Color32::TRANSPARENT),
            ] {
                fill.colored_vertex(pos, color);
            }
            fill.add_triangle(base, base + 1, base + 2);
            fill.add_triangle(base, base + 2, base + 3);
        }
        painter.add(egui::Shape::mesh(fill));
        // The line itself: one stroke, one colour, no second series.
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255)),
        ));
        painter.text(
            rect.left_top() + egui::vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!("{} {:.1}", label, data.last().copied().unwrap_or(0.0)),
            egui::FontId::proportional(12.0),
            Color32::from_rgb(205, 225, 255),
        );
    }

    pub(super) fn priority_window(&mut self, ctx: &egui::Context) {
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

    pub(super) fn startup_tab(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn users_tab(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn network_tab(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn services_tab(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn settings_tab(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn confirm_modal(&mut self, ctx: &egui::Context) {
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

/// One collapsible group caption: `▾ Приложения (13)` plus the aggregate CPU and
/// memory of everything inside.  Clicking the caption toggles the group.
/// One group header: a checkbox that shows the group, its name and size, and
/// the group's aggregate CPU and memory.
///
/// The affordance used to be a bare chevron, so nothing on screen said whether
/// the group was "included" in the view - the review asked for a checkbox, and
/// a checkbox is also what Windows shows.  Clicking the box or the name
/// toggles the group.
fn group_header(ui: &mut egui::Ui, title: &str, rows: &[&Row], collapsed: &mut bool) {
    let cpu: f32 = rows.iter().map(|r| r.cpu_percent).sum();
    let mem: u64 = rows.iter().map(|r| r.info.private_bytes).sum();
    ui.horizontal(|ui| {
        // `shown` is the opposite of `collapsed`: a ticked box means the rows
        // are part of the list.
        let mut shown = !*collapsed;
        let box_resp = ui.checkbox(&mut shown, "");
        if box_resp.changed() {
            *collapsed = !shown;
        }
        let arrow = if *collapsed { "▸" } else { "▾" };
        let caption = format!("{arrow} {title} ({})", rows.len());
        let resp =
            ui.add(egui::Button::new(RichText::new(caption).strong().size(12.5)).frame(false));
        if resp.clicked() {
            *collapsed = !*collapsed;
        }
        ui.label(RichText::new(format!("CPU {:.1}%", cpu)).small().weak());
        ui.label(RichText::new(mb(mem)).small().weak());
    });
}

fn enumerate_sessions() -> Vec<String> {
    let console = std::env::var("SESSIONNAME").unwrap_or_else(|_| "Console".into());
    vec![format!("  {console} — {}", ts("active"))]
}

//! WinRust File Explorer.
//!
//! A complete file browser: sidebar, tabs, breadcrumb address bar, details /
//! tiles / list views, sorting, context menu (open, edit, run as admin,
//! properties, recycle), file operations, search, and per-folder view state
//! persisted in `%APPDATA%\WinRust`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 420.0])
            .with_title("WinRust Explorer"),
        ..Default::default()
    };
    eframe::run_native(
        "WinRust Explorer",
        options,
        Box::new(|cc| Ok(Box::new(app::ExplorerApp::new(cc)))),
    )
}

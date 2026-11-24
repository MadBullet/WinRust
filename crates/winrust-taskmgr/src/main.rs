//! WinRust Task Manager.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("WinRust Task Manager"),
        ..Default::default()
    };
    eframe::run_native(
        "WinRust Task Manager",
        options,
        Box::new(|cc| Ok(Box::new(app::TaskManagerApp::new(cc)))),
    )
}

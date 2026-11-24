//! WinRust System Control.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 740.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("WinRust System Control"),
        ..Default::default()
    };
    eframe::run_native(
        "WinRust System Control",
        options,
        Box::new(|cc| Ok(Box::new(app::SysCtlApp::new(cc)))),
    )
}

//! WinRust Services Manager.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let appearance = winrust_core::i18n::settings::load();
    let ru = winrust_core::i18n::lang::load() == winrust_core::i18n::Lang::Ru;
    let title = if ru {
        "Службы WinRust"
    } else {
        "WinRust Services"
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([860.0, 520.0])
            .with_title(title)
            .with_transparent(appearance.transparent),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(|cc| Ok(Box::new(app::ServicesApp::new(cc)))),
    )
}

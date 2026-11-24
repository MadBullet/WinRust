//! WinRust Task Manager.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let appearance = winrust_core::i18n::settings::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([860.0, 480.0])
            .with_title(if winrust_core::i18n::lang::load() == winrust_core::i18n::Lang::Ru {
                "Диспетчер задач WinRust"
            } else {
                "WinRust Task Manager"
            })
            // Transparency has to be requested when the window is created;
            // the switch in Settings then only changes the alpha (see theme.rs).
            .with_transparent(appearance.transparent),
        ..Default::default()
    };
    let title = if winrust_core::i18n::lang::load() == winrust_core::i18n::Lang::Ru {
        "Диспетчер задач WinRust"
    } else {
        "WinRust Task Manager"
    };
    eframe::run_native(
        title,
        options,
        Box::new(|cc| Ok(Box::new(app::TaskManagerApp::new(cc)))),
    )
}

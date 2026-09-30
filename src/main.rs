#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod geometry;
mod hotkeys;
mod image_parser;
mod messages;
mod mouse;
mod trajectory_optimizer;
mod worker;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .init();

    log::info!("==== img2spire 启动 ====");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 640.0])
            .with_title("img2spire"),
        ..Default::default()
    };

    eframe::run_native(
        "img2spire",
        options,
        Box::new(|cc| Ok(Box::new(app::AutoDrawerApp::new(cc)))),
    )
}

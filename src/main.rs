#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod image_parser;
mod messages;
mod trajectory_optimizer;
mod worker;

use eframe::egui;

fn main() -> eframe::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    log::info!("==== img2spire 祈动 ====");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_transparent(true)
            .with_title("img2spire"),
        ..Default::default()
    };

    eframe::run_native(
        "img2spire",
        options,
        Box::new(|cc| Ok(Box::new(app::AutoDrawerApp::new(cc)))),
    )
}

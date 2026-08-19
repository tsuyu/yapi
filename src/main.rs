#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chain;
mod extract;
mod jsonpath;
mod jwt;
mod model;
mod net;
mod oauth;
mod pretty;
mod store;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 800.0])
            .with_min_inner_size([820.0, 540.0])
            .with_title("api-req"),
        ..Default::default()
    };
    eframe::run_native(
        "api-req",
        options,
        Box::new(|cc| Ok(Box::new(app::ApiReqApp::new(cc)))),
    )
}

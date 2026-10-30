//! WineWrap desktop app: a CrossOver-style window for managing bottles,
//! installing Steam and launching games.

// Hide the console window on Windows release builds (harmless elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod jobs;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("WineWrap")
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([720.0, 480.0]);
    // The .app bundle has its own icon; this one shows when running unbundled
    // (`cargo run`), in the Dock and window switcher.
    match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => tracing::warn!("could not load the app icon: {e}"),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "WineWrap",
        options,
        Box::new(|cc| Ok(Box::new(app::WineWrapApp::new(cc)))),
    )
}

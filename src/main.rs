#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod launcher;
mod mod_manager;
mod scanner;

use app::SekaiHunterApp;
#[cfg(target_os = "linux")]
use winit::platform::x11::EventLoopBuilderExtX11;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    log::info!("Sekai Hunter v0.1.0 starting");

    let config = config::Config::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sekai Hunter")
            .with_inner_size([
                config.window_width.max(820.0),
                config.window_height.max(580.0),
            ])
            .with_min_inner_size([680.0, 500.0])
            .with_drag_and_drop(true),
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "linux")]
            {
                if std::env::var_os("WAYLAND_DISPLAY").is_some()
                    && std::env::var_os("DISPLAY").is_some()
                {
                    log::info!("Using XWayland to support file drag and drop.");
                    builder.with_x11();
                }
            }
        })),
        ..Default::default()
    };

    eframe::run_native(
        "Sekai Hunter",
        options,
        Box::new(|cc| Ok(Box::new(SekaiHunterApp::new(cc)))),
    )
}

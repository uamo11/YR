use std::path::PathBuf;
use yr_app::YrApp;

fn main() -> eframe::Result {
    let data_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/yr-music");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([430.0, 820.0])
            .with_title("YR Music"),
        ..Default::default()
    };

    eframe::run_native(
        "YR Music",
        options,
        Box::new(move |cc| Ok(Box::new(YrApp::new(cc, data_dir)))),
    )
}

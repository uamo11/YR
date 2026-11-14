pub mod app;
pub mod theme;

pub use app::YrApp;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    use std::path::PathBuf;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("Starting YR Music on Android");

    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/yr_music"));

    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };

    if let Err(err) = eframe::run_native(
        "YR Music",
        options,
        Box::new(move |cc| Ok(Box::new(YrApp::new(cc, data_dir)))),
    ) {
        log::error!("eframe error: {err:?}");
    }
}

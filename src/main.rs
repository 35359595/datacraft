#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod db;
mod edit;
mod i18n;
mod native_menu;
mod sql;
mod ui;
mod worker;

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DataCraft")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            // Title bar unified with the toolbar (DataGrip style): content is drawn under
            // the transparent bar and only the macOS traffic lights remain.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_icon(ui::branding::icon_data()),
        ..Default::default()
    };
    // Demo mode (see app.rs): don't touch the user's persisted state.
    if std::env::var("DATACRAFT_DEMO").is_ok_and(|v| !v.is_empty()) {
        options.persistence_path = Some(std::env::temp_dir().join("datacraft-demo-state"));
        options.persist_window = false;
    }
    eframe::run_native("DataCraft", options, Box::new(|cc| Ok(Box::new(app::DataCraftApp::new(cc)))))
}

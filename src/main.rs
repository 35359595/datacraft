#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod db;
mod edit;
mod sql;
mod ui;
mod worker;

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DataCraft")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 560.0])
            // Barra de título unificada con la toolbar (estilo DataGrip): se dibuja el contenido
            // bajo la barra transparente y solo quedan los semáforos de macOS.
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        ..Default::default()
    };
    // Modo demo (ver app.rs): no tocar el estado persistido del usuario.
    if std::env::var("DATACRAFT_DEMO").is_ok_and(|v| !v.is_empty()) {
        options.persistence_path = Some(std::env::temp_dir().join("datacraft-demo-state"));
        options.persist_window = false;
    }
    eframe::run_native("DataCraft", options, Box::new(|cc| Ok(Box::new(app::DataCraftApp::new(cc)))))
}

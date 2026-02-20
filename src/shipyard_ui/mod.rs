mod app;
mod config;
mod layout;
mod runtime;
mod state;
mod theme;
mod widgets;

pub fn launch() -> Result<(), String> {
    let native_options = config::native_options();

    eframe::run_native(
        config::WINDOW_TITLE,
        native_options,
        Box::new(|creation_context| {
            theme::configure_theme(&creation_context.egui_ctx);
            Ok(Box::new(app::ShipyardApp::new()))
        }),
    )
    .map_err(|error| error.to_string())
}

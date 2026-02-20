mod app;
mod config;
mod runtime;
mod state;
mod ui;

pub fn launch() -> Result<(), String> {
    app::launch()
}

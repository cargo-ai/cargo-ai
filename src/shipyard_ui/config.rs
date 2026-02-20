use eframe::egui;

pub const WINDOW_TITLE: &str = "Cargo AI Shipyard";
pub const WINDOW_INITIAL_WIDTH: f32 = 1180.0;
pub const WINDOW_INITIAL_HEIGHT: f32 = 760.0;
pub const WINDOW_MIN_WIDTH: f32 = 900.0;
pub const WINDOW_MIN_HEIGHT: f32 = 560.0;

pub const REPAINT_INTERVAL_MS: u64 = 40;
pub const LOW_DPI_PPP_THRESHOLD: f32 = 1.35;
pub const LOW_DPI_ZOOM_FACTOR: f32 = 1.1;

pub const TITLE_PANEL_HEIGHT: f32 = 52.0;
pub const EXECUTION_PANEL_MIN_HEIGHT: f32 = 170.0;
pub const EXECUTION_PANEL_DEFAULT_RATIO: f32 = 0.33;
pub const EXECUTION_PANEL_MAX_RATIO: f32 = 0.55;
pub const EXECUTION_PANEL_PERSIST_WRITE_THRESHOLD: f32 = 1.0;

pub const TERMINAL_FONT_SIZE: f32 = 12.5;
pub const TERMINAL_CORNER_RADIUS: u8 = 8;
pub const WORKSPACE_CORNER_RADIUS: u8 = 12;
pub const TITLE_LOGO_HEIGHT: f32 = 24.0;
pub const WORKSPACE_LOGO_HEIGHT: f32 = 140.0;
pub const WORKSPACE_LOGO_ALPHA: u8 = 50;

pub const ACCOUNT_STATUS_INTENT_LABEL: &str = "Run `account status`";
pub const ACCOUNT_STATUS_VERBOSE_ARGS: &[&str] = &["account", "status"];
pub const ACCOUNT_REGISTER_VERBOSE_PREFIX_ARGS: &[&str] = &["account", "register"];
pub const ACCOUNT_CONFIRM_VERBOSE_PREFIX_ARGS: &[&str] = &["account", "confirm"];

pub fn execution_panel_default_height(viewport_height: f32) -> f32 {
    let max_height = execution_panel_max_height(viewport_height);
    (viewport_height * EXECUTION_PANEL_DEFAULT_RATIO)
        .max(EXECUTION_PANEL_MIN_HEIGHT)
        .min(max_height)
}

pub fn execution_panel_max_height(viewport_height: f32) -> f32 {
    (viewport_height * EXECUTION_PANEL_MAX_RATIO).max(EXECUTION_PANEL_MIN_HEIGHT)
}

pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([WINDOW_INITIAL_WIDTH, WINDOW_INITIAL_HEIGHT])
            .with_min_inner_size([WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT]),
        ..Default::default()
    }
}

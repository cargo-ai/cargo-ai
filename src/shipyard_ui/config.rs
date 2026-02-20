use eframe::egui;

pub const WINDOW_TITLE: &str = "Cargo AI Shipyard";
pub const WINDOW_INITIAL_WIDTH: f32 = 1180.0;
pub const WINDOW_INITIAL_HEIGHT: f32 = 760.0;
pub const WINDOW_MIN_WIDTH: f32 = 900.0;
pub const WINDOW_MIN_HEIGHT: f32 = 560.0;

pub const REPAINT_INTERVAL_MS: u64 = 40;

pub const TITLE_PANEL_HEIGHT: f32 = 52.0;
pub const EXECUTION_PANEL_DEFAULT_HEIGHT: f32 = 250.0;
pub const EXECUTION_PANEL_MIN_HEIGHT: f32 = 170.0;
pub const EXECUTION_PANEL_TARGET_RATIO: f32 = 0.33;

pub const TERMINAL_FONT_SIZE: f32 = 12.5;
pub const TERMINAL_CORNER_RADIUS: u8 = 8;
pub const WORKSPACE_CORNER_RADIUS: u8 = 12;

pub const PROFILE_LIST_INTENT_LABEL: &str = "Run `profile list`";
pub const PROFILE_LIST_VERBOSE_ARGS: &[&str] = &["profile", "list"];

pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([WINDOW_INITIAL_WIDTH, WINDOW_INITIAL_HEIGHT])
            .with_min_inner_size([WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT]),
        ..Default::default()
    }
}
